use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;
use tauri::Manager;

mod supervisor;
use supervisor::{unix_ms, SessionObservation, SupervisorSnapshot, SupervisorStore};

/// Loopback window the app is allowed to look in. perfect-planning's default board port is
/// 5230 and it steps upward when that is taken, so a small window covers every real case —
/// and clamping here means the webview can never talk this command into a port sweep.
const WINDOW_START: u16 = 5200;
const WINDOW_END: u16 = 5299;

/// Read one of the two explicitly allowed board endpoints.
///
/// The board server 403s a foreign `Origin` and demands an exact `Host`, so this speaks raw
/// HTTP/1.0: no Origin at all, the Host it expects, and a bounded reply that ends when the
/// server closes. Callers cannot provide `path`, so this can never become a general-purpose
/// loopback HTTP client.
fn request_json(port: u16, path: &'static str) -> Option<Value> {
    debug_assert!(matches!(path, "/whoami" | "/workers" | "/plan"));
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(700)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(700)))
        .ok()?;

    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut raw = Vec::new();
    (&mut stream).take(64 * 1024).read_to_end(&mut raw).ok()?;
    parse_json_response(&raw)
}

/// Send one narrowly-scoped supervisor event back to the exact board that produced the
/// observation. This is the durable half of reaping: the app-local lease is not enough,
/// because `/workers` is derived from the plan and would otherwise report the dead owner
/// forever after every restart.
fn post_recovery_event(port: u16, event: &supervisor::ReaperEvent) -> Result<Value, String> {
    let path = "/recover-stale";
    let body = serde_json::to_vec(event)
        .map_err(|error| format!("cannot serialize session recovery event: {error}"))?;
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200))
        .map_err(|error| format!("cannot connect to board on port {port}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(700)))
        .map_err(|error| format!("cannot set board read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(700)))
        .map_err(|error| format!("cannot set board write timeout: {error}"))?;

    let request = format!(
        "POST {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Plan-Mtime: desktop-supervisor\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|error| format!("cannot send session recovery event: {error}"))?;

    let mut raw = Vec::new();
    (&mut stream)
        .take(64 * 1024)
        .read_to_end(&mut raw)
        .map_err(|error| format!("cannot read session recovery response: {error}"))?;
    let (status, response) = parse_json_response_any(&raw)
        .ok_or_else(|| "board returned an invalid session recovery response".to_string())?;
    if status != 200 {
        let message = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("board refused session recovery");
        return Err(format!("board returned HTTP {status}: {message}"));
    }
    Ok(response)
}

fn parse_json_response(raw: &[u8]) -> Option<Value> {
    let (status, body) = parse_json_response_any(raw)?;
    (status == 200).then_some(body)
}

fn parse_json_response_any(raw: &[u8]) -> Option<(u16, Value)> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, serde_json::from_str(body.trim()).ok()?))
}

fn probe_identity(port: u16) -> Option<Value> {
    let mut parsed = request_json(port, "/whoami")?;
    if parsed.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    // Trust the port we actually reached over the one the payload claims.
    if let Some(obj) = parsed.as_object_mut() {
        obj.insert("port".to_string(), Value::from(port));
    }
    Some(parsed)
}

/// Every perfect-planning board currently serving on loopback, newest port last.
///
/// Discovery reuses the skill's own `/whoami` identity check. It never starts a server:
/// a competing board on the same plan is exactly what that check exists to prevent.
#[tauri::command]
fn discover_boards(start: u16, end: u16) -> Vec<Value> {
    let lo = start.max(WINDOW_START);
    let hi = end.min(WINDOW_END);
    if lo > hi {
        return Vec::new();
    }

    let handles: Vec<_> = (lo..=hi)
        .map(|port| std::thread::spawn(move || probe_identity(port)))
        .collect();

    let mut boards: Vec<Value> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();

    boards.sort_by_key(|b| b.get("port").and_then(Value::as_u64).unwrap_or(0));
    boards
}

/// Return heartbeat state only after the port still proves it serves the plan the UI found.
/// The identity re-check closes the port-reuse race between discovery and this request.
#[tauri::command]
fn read_board_workers(port: u16, plan_path: String) -> Option<Value> {
    if !(WINDOW_START..=WINDOW_END).contains(&port) || plan_path.is_empty() {
        return None;
    }
    let identity = probe_identity(port)?;
    if identity.get("planPath").and_then(Value::as_str) != Some(plan_path.as_str()) {
        return None;
    }
    let snapshot = request_json(port, "/workers")?;
    snapshot.get("workers")?.as_object()?;
    Some(snapshot)
}

/// Read the plan manifest after the same identity fence. This stays separate from worker state
/// so collision mapping gains file/resource metadata without gaining any write path.
#[tauri::command]
fn read_board_plan(port: u16, plan_path: String) -> Option<Value> {
    if !(WINDOW_START..=WINDOW_END).contains(&port) || plan_path.is_empty() {
        return None;
    }
    let identity = probe_identity(port)?;
    if identity.get("planPath").and_then(Value::as_str) != Some(plan_path.as_str()) {
        return None;
    }
    let plan = request_json(port, "/plan")?;
    plan.get("vertebrae")?.as_array()?;
    Some(plan)
}

/// Return one proof artifact after proving both the board identity and a single-file path.
/// Reading from disk avoids keeping another browser alive and avoids weakening the board's
/// foreign-Origin guard merely to make screenshots visible in the desktop webview.
#[tauri::command]
fn read_board_evidence(port: u16, plan_path: String, file_name: String) -> Option<Value> {
    if !(WINDOW_START..=WINDOW_END).contains(&port)
        || plan_path.is_empty()
        || file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return None;
    }
    let identity = probe_identity(port)?;
    if identity.get("planPath").and_then(Value::as_str) != Some(plan_path.as_str()) {
        return None;
    }
    let evidence_dir = Path::new(&plan_path).parent()?.join("evidence").canonicalize().ok()?;
    let file = evidence_dir.join(&file_name).canonicalize().ok()?;
    if file.parent()? != evidence_dir || !file.is_file() {
        return None;
    }
    let bytes = fs::read(&file).ok()?;
    if bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    let ext = file.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "log" | "txt" | "jsonl" => "text/plain",
        "md" => "text/markdown",
        _ => "application/octet-stream",
    };
    if mime.starts_with("text/") {
        Some(json!({ "name": file_name, "mime": mime, "text": String::from_utf8_lossy(&bytes) }))
    } else {
        Some(json!({ "name": file_name, "mime": mime, "dataBase64": BASE64_STANDARD.encode(bytes) }))
    }
}

/// Reconcile board observations into the app-local lease registry. This never edits a plan or
/// kills a process: it releases only claims held by this supervisor, and only after the reaper
/// has durably journaled the transition.
#[tauri::command]
fn reconcile_session_leases(
    state: tauri::State<'_, SupervisorStore>,
    observations: Vec<SessionObservation>,
) -> Result<SupervisorSnapshot, String> {
    state.observe(observations, unix_ms())
}

#[tauri::command]
fn supervisor_snapshot(
    state: tauri::State<'_, SupervisorStore>,
) -> Result<SupervisorSnapshot, String> {
    state.snapshot(unix_ms())
}

/// Mirror an already-durable SESSION_CLEARED event into the serving plan. Identity is
/// checked immediately before the write, closing the port-reuse/wrong-organization race.
#[tauri::command]
fn recover_board_session(
    port: u16,
    plan_path: String,
    event: supervisor::ReaperEvent,
) -> Result<Value, String> {
    if !(WINDOW_START..=WINDOW_END).contains(&port) || plan_path.trim().is_empty() {
        return Err("invalid board recovery target".to_string());
    }
    if event.plan_path != plan_path {
        return Err("session recovery event names a different plan".to_string());
    }
    let identity = probe_identity(port)
        .ok_or_else(|| format!("board identity unavailable on port {port}"))?;
    if identity.get("planPath").and_then(Value::as_str) != Some(plan_path.as_str()) {
        return Err("board identity changed before session recovery".to_string());
    }
    post_recovery_event(port, &event)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let ledger_path = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("cannot resolve supervisor data directory: {error}"))?
                .join("session-reaper.jsonl");
            let supervisor = SupervisorStore::open(ledger_path)?;
            supervisor.spawn_reaper()?;
            app.manage(supervisor);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            discover_boards,
            read_board_workers,
            read_board_plan,
            read_board_evidence,
            reconcile_session_leases,
            supervisor_snapshot,
            recover_board_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running perfect planner desktop");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whoami_requires_success_status_and_ok_identity() {
        let good =
            b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true,\"port\":9}";
        let parsed = parse_json_response(good).expect("valid JSON response");
        assert_eq!(parsed["port"], 9);

        let wrong_status = b"HTTP/1.0 404 Not Found\r\n\r\n{\"ok\":true}";
        assert!(parse_json_response(wrong_status).is_none());

        let malformed = b"HTTP/1.0 200 OK\r\n\r\nnot-json";
        assert!(parse_json_response(malformed).is_none());

        let conflict = b"HTTP/1.0 409 Conflict\r\nContent-Type: application/json\r\n\r\n{\"ok\":false,\"error\":\"active\"}";
        let (status, body) = parse_json_response_any(conflict).expect("error JSON response");
        assert_eq!(status, 409);
        assert_eq!(body["error"], "active");
    }

    #[test]
    fn discovery_refuses_ranges_outside_the_board_window() {
        assert!(discover_boards(5300, 5400).is_empty());
        assert!(discover_boards(5000, 5100).is_empty());
    }
}
