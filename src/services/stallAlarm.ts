const MIN_GAIN = 0.0001;
const ALARM_SECONDS = 2.8;

/**
 * One soft, slowly rising signal. It is synthesized locally so alerts work offline and do
 * not need a media asset, decoder, or another process. The caller owns transition de-duping.
 */
export async function playRisingAlarm(volume: number): Promise<boolean> {
  const AudioContextClass = window.AudioContext;
  if (!AudioContextClass) return false;

  const context = new AudioContextClass();
  try {
    if (context.state === "suspended") await context.resume();
    if (context.state !== "running") {
      await context.close();
      return false;
    }

    const now = context.currentTime;
    const peak = Math.max(0.01, Math.min(1, volume)) * 0.18;
    const oscillator = context.createOscillator();
    const gain = context.createGain();

    oscillator.type = "sine";
    oscillator.frequency.setValueAtTime(330, now);
    oscillator.frequency.exponentialRampToValueAtTime(660, now + 2.35);

    gain.gain.setValueAtTime(MIN_GAIN, now);
    gain.gain.exponentialRampToValueAtTime(peak, now + 2.15);
    gain.gain.setValueAtTime(peak, now + 2.35);
    gain.gain.exponentialRampToValueAtTime(MIN_GAIN, now + ALARM_SECONDS);

    oscillator.connect(gain);
    gain.connect(context.destination);
    oscillator.start(now);
    oscillator.stop(now + ALARM_SECONDS);
    oscillator.addEventListener("ended", () => void context.close(), { once: true });
    return true;
  } catch {
    await context.close().catch(() => undefined);
    return false;
  }
}

export const alarmDurationMs = ALARM_SECONDS * 1000;
