import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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

  const fetchEvents = () => {
    invoke<AgendaEvent[]>("fetch_todays_events")
      .then((data) => setEvents(data))
      .catch((err) => console.error("Failed to fetch calendar agenda:", err));
  };

  useEffect(() => {
    fetchEvents();
    const interval = setInterval(fetchEvents, 60 * 1000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="schedule-widget">
      <div className="schedule-header">
        <span className="schedule-title">Schedule</span>
      </div>

      <div className="schedule-events-list">
        {events.length === 0 ? (
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