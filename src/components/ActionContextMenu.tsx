import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

export type ContextActionLog = {
  source: "context-menu";
  message: string;
  level: "info" | "warning";
};

type ContextKind = "plan" | "modal" | "node" | "evidence" | "expandable" | "option" | "surface";

interface MenuTarget {
  element: HTMLElement;
  kind: ContextKind;
  id: string;
  label: string;
  x: number;
  y: number;
}

interface MenuAction {
  id: string;
  label: string;
  disabled?: boolean;
  danger?: boolean;
  title?: string;
}

export interface ActionContextMenuProps {
  onPlanAction: (action: "select" | "remove" | "open", planPath: string) => void;
  onLog: (entry: ContextActionLog) => void;
}

const TARGET_SELECTOR = [
  "[data-context-kind]",
  "[role='dialog']",
  "[data-evidence-id]",
  "[data-node-id]",
  "details",
  "button",
  "a",
  "[role='button']",
  "[data-context-surface]",
].join(",");

function inferredKind(element: HTMLElement): ContextKind {
  const declared = element.dataset.contextKind as ContextKind | undefined;
  if (declared) return declared;
  if (element.matches("[role='dialog']")) return "modal";
  if (element.matches("[data-evidence-id]")) return "evidence";
  if (element.matches("[data-node-id]")) return "node";
  if (element.matches("details")) return "expandable";
  if (element.matches("button, a, [role='button']")) return "option";
  return "surface";
}

function targetLabel(element: HTMLElement): string {
  return (
    element.dataset.contextLabel ||
    element.getAttribute("aria-label") ||
    element.getAttribute("title") ||
    element.textContent ||
    "Selected item"
  ).replace(/\s+/g, " ").trim().slice(0, 96);
}

function targetId(element: HTMLElement): string {
  return (
    element.dataset.contextId ||
    element.dataset.planPath ||
    element.dataset.nodeId ||
    element.dataset.evidenceId ||
    element.id ||
    targetLabel(element)
  );
}

function actionsFor(kind: ContextKind): MenuAction[] {
  if (kind === "plan") {
    return [
      { id: "select", label: "Open plan" },
      { id: "open", label: "Open board in browser" },
      { id: "copy", label: "Copy plan identity" },
      { id: "remove", label: "Remove from rail", danger: true },
      {
        id: "reject",
        label: "Reject and delete",
        danger: true,
        disabled: true,
        title: "Blocked until the planner engine requires an explicit yes before every execution",
      },
    ];
  }
  if (kind === "modal") {
    return [
      { id: "focus", label: "Bring modal into view" },
      { id: "copy", label: "Copy modal identity" },
      { id: "close", label: "Close modal" },
    ];
  }
  if (kind === "node" || kind === "expandable") {
    return [
      { id: "toggle", label: "Expand / collapse" },
      { id: "focus", label: "Bring into view" },
      { id: "copy", label: kind === "node" ? "Copy node identity" : "Copy identity" },
    ];
  }
  if (kind === "evidence") {
    return [
      { id: "focus", label: "Bring evidence into view" },
      { id: "copy", label: "Copy evidence identity" },
    ];
  }
  if (kind === "option") {
    return [
      { id: "activate", label: "Use this option" },
      { id: "copy", label: "Copy option label" },
    ];
  }
  return [
    { id: "focus", label: "Bring into view" },
    { id: "copy", label: "Copy identity" },
  ];
}

async function copyText(value: string): Promise<void> {
  if (!navigator.clipboard?.writeText) throw new Error("Clipboard access is unavailable");
  await navigator.clipboard.writeText(value);
}

export function ActionContextMenu({ onPlanAction, onLog }: ActionContextMenuProps) {
  const [target, setTarget] = useState<MenuTarget | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const actions = useMemo(() => target ? actionsFor(target.kind) : [], [target]);

  useEffect(() => {
    const open = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || event.target.closest("#pp-context-menu")) return;
      const element = event.target.closest(TARGET_SELECTOR);
      if (!(element instanceof HTMLElement)) return;
      event.preventDefault();
      const x = Math.max(8, Math.min(event.clientX, window.innerWidth - 280));
      const y = Math.max(8, Math.min(event.clientY, window.innerHeight - 330));
      setTarget({ element, kind: inferredKind(element), id: targetId(element), label: targetLabel(element), x, y });
    };
    const dismiss = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest("#pp-context-menu")) setTarget(null);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setTarget(null);
    };
    const blur = () => setTarget(null);
    document.addEventListener("contextmenu", open, true);
    document.addEventListener("mousedown", dismiss, true);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("blur", blur);
    return () => {
      document.removeEventListener("contextmenu", open, true);
      document.removeEventListener("mousedown", dismiss, true);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("blur", blur);
    };
  }, []);

  useEffect(() => {
    if (target) menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [target]);

  if (!target) return null;

  const runAction = async (action: MenuAction) => {
    if (action.disabled) return;
    const element = target.element;
    try {
      if (target.kind === "plan" && ["select", "remove", "open"].includes(action.id)) {
        onPlanAction(action.id as "select" | "remove" | "open", element.dataset.planPath || target.id);
      } else if (action.id === "copy") {
        await copyText(target.id);
      } else if (action.id === "activate") {
        element.click();
      } else if (action.id === "focus") {
        element.scrollIntoView({ behavior: "smooth", block: "center", inline: "nearest" });
        element.focus({ preventScroll: true });
      } else if (action.id === "toggle") {
        const details = element.matches("details") ? element : element.closest("details");
        if (details instanceof HTMLDetailsElement) details.open = !details.open;
      } else if (action.id === "close") {
        const selector = element.dataset.contextClose;
        const close = selector
          ? element.querySelector<HTMLElement>(selector)
          : element.querySelector<HTMLElement>("[aria-label*='close' i], [id*='close' i]");
        close?.click();
      }
      onLog({ source: "context-menu", level: "info", message: `${action.label}: ${target.label}` });
    } catch (error) {
      onLog({ source: "context-menu", level: "warning", message: `${action.label} failed: ${error instanceof Error ? error.message : String(error)}` });
    } finally {
      setTarget(null);
    }
  };

  return createPortal(
    <div
      className="pp-context-menu"
      id="pp-context-menu"
      ref={menuRef}
      role="menu"
      aria-label={`Actions for ${target.label}`}
      style={{ left: target.x, top: target.y }}
      onKeyDown={(event) => {
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          const step = event.key === "ArrowDown" ? 1 : -1;
          buttons[(index + step + buttons.length) % buttons.length]?.focus();
        }
      }}
    >
      <header>
        <span>{target.kind}</span>
        <strong>{target.label}</strong>
      </header>
      {actions.map((action) => (
        <button
          key={action.id}
          type="button"
          role="menuitem"
          className={action.danger ? "danger" : undefined}
          disabled={action.disabled}
          title={action.title}
          onClick={() => void runAction(action)}
        >
          {action.label}
          {action.disabled ? <small>blocked</small> : null}
        </button>
      ))}
      <footer>Right-click anywhere else to switch target · Esc closes</footer>
    </div>,
    document.body
  );
}
