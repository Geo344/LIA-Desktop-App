import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface AgendaEvent {
  calendar_id: string;
  calendar_name: string;
  event_id: string;
  title: string;
  start_time: string;
  end_time: string;
  starts_in_ten: boolean;
  is_in_progress: boolean;
  is_all_day: boolean;
}

interface CalendarAgenda {
  events: AgendaEvent[];
  unavailable_calendars: string[];
}

export default function ScheduleWidget() {
  const [events, setEvents] = useState<AgendaEvent[]>([]);
  const [unavailableCalendars, setUnavailableCalendars] = useState<string[]>([]);
  const [status, setStatus] = useState("");
  const [hasLoaded, setHasLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [canReconnect, setCanReconnect] = useState(false);
  const [attempt, setAttempt] = useState({ id: 0, reconnect: false });

  const retry = (reconnect = false) => {
    setError(null);
    setCanReconnect(false);
    setStatus(reconnect ? "Resetting Google access…" : "");
    setAttempt((value) => ({ id: value.id + 1, reconnect }));
  };

  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    let failed = false;
    let interval: ReturnType<typeof setInterval> | undefined;
    let unlisten: (() => void) | undefined;
    let unlistenReconnect: (() => void) | undefined;
    let reconnectRequested = attempt.reconnect;
    const stopListeners = () => {
      unlisten?.();
      unlisten = undefined;
      unlistenReconnect?.();
      unlistenReconnect = undefined;
    };

    const fetchEvents = async () => {
      if (disposed || inFlight || failed) return;
      inFlight = true;
      setError(null);
      setCanReconnect(false);
      setStatus(reconnectRequested ? "Resetting Google access…" : "");
      try {
        const command = reconnectRequested ? "reconnect_google_calendar" : "fetch_todays_events";
        reconnectRequested = false; // Never repeat a reset on subsequent polls.
        const data = await invoke<CalendarAgenda>(command);
        if (!disposed) {
          setEvents(data.events);
          setUnavailableCalendars(data.unavailable_calendars);
          setHasLoaded(true);
        }
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
    void (async () => {
      const stopListening = await listen("calendar-auth-waiting", () => {
        if (!disposed && inFlight) setStatus("Waiting for Google sign-in…");
      });
      if (disposed) {
        stopListening();
        return;
      }
      unlisten = stopListening;
      const stopReconnect = await listen("calendar-reconnect-required", () => {
        if (!disposed) setCanReconnect(true);
      });
      if (disposed) {
        stopReconnect();
        return;
      }
      unlistenReconnect = stopReconnect;
      void fetchEvents();
      interval = setInterval(() => void fetchEvents(), 60 * 1000);
    })().catch((err) => {
      stopListeners();
      if (!disposed) {
        setStatus("");
        setError(String(err));
      }
    });

    return () => {
      disposed = true;
      if (interval !== undefined) clearInterval(interval);
      stopListeners();
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
            {error && <button className="schedule-retry" onClick={() => retry()}>Retry</button>}
            {error && canReconnect && (
              <>
                <span>Reconnect removes Google access for every OAuth client in LIA's Google Cloud project, then opens a new sign-in.</span>
                <button className="schedule-retry" onClick={() => retry(true)}>Reconnect Google</button>
              </>
            )}
          </div>
        )}
        {unavailableCalendars.length > 0 && (
          <div className="schedule-status" role="status">
            Could not read: {unavailableCalendars.join(", ")}. Showing available calendars.
          </div>
        )}
        {hasLoaded && events.length === 0 && !status && !error && unavailableCalendars.length === 0 ? (
          <div className="schedule-empty">No remaining events today</div>
        ) : (
          events.map((event, idx) => (
            <div
              key={`${event.calendar_id}:${event.event_id || idx}`}
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
