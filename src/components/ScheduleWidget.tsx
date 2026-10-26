import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface AgendaEvent {
  title: string;
  start_time: string;
  end_time: string;
  starts_in_ten: boolean;
  is_in_progress: boolean;
  is_all_day: boolean;
}

export default function ScheduleWidget() {
  const [events, setEvents] = useState<AgendaEvent[]>([]);
  const [status, setStatus] = useState("Loading schedule…");
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    let failed = false;
    let interval: ReturnType<typeof setInterval> | undefined;
    let unlisten: (() => void) | undefined;

    const fetchEvents = async () => {
      if (disposed || inFlight || failed) return;
      inFlight = true;
      setError(null);
      setStatus("Loading schedule…");
      try {
        const data = await invoke<AgendaEvent[]>("fetch_todays_events");
        if (!disposed) setEvents(data);
      } catch (err) {
        // Wait for an explicit retry instead of reopening sign-in every minute.
        failed = true;
        if (!disposed) setError(String(err));
      } finally {
        inFlight = false;
        if (!disposed) setStatus("");
      }
    };

    // Subscribe before invoking so a fast authentication event cannot be missed.
    void listen("calendar-auth-waiting", () => {
      if (!disposed && inFlight) setStatus("Waiting for Google sign-in…");
    }).then((stopListening) => {
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      void fetchEvents();
      interval = setInterval(() => void fetchEvents(), 60 * 1000);
    }).catch((err) => {
      if (!disposed) {
        setStatus("");
        setError(String(err));
      }
    });

    return () => {
      disposed = true;
      if (interval !== undefined) clearInterval(interval);
      unlisten?.();
    };
  }, [attempt]);

  return (
    <div className="schedule-widget">
      <div className="schedule-header">
        <span className="schedule-title">Schedule</span>
      </div>

      <div className="schedule-events-list">
        {(status || error) && (
          <div className="schedule-status" role="status" aria-live="polite">
            <span>{error || status}</span>
            {error && <button className="schedule-retry" onClick={() => {
              setError(null);
              setStatus("Loading schedule…");
              setAttempt((value) => value + 1);
            }}>Retry</button>}
          </div>
        )}
        {events.length === 0 && !status && !error ? (
          <div className="schedule-empty">No remaining events today</div>
        ) : (
          events.map((event, idx) => (
            <div
              key={idx}
              className={`schedule-event-row ${event.is_in_progress ? "in-progress" : ""}`}
            >
              {event.starts_in_ten && !event.is_in_progress && (
                <span className="schedule-warning-dot" title="Starting in 10 minutes or less!" />
              )}
              <span className="schedule-event-time">
                {event.is_all_day ? "All Day" : event.start_time}
              </span>
              <span className="schedule-event-name" title={event.title}>
                {event.title}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
