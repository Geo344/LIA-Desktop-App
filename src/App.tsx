import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import wallpaperImg from "./assets/Vivy_Wallpaper.png";

// Customized SVG Icons
import TrashIcon from "./assets/icons/Trash.svg";
import FolderIcon from "./assets/icons/Folder.svg";
import BooksIcon from "./assets/icons/Books.svg";
import FinanceIcon from "./assets/icons/Finance.svg";
import ZoomIcon from "./assets/icons/Zoom.svg";
import DiscordIcon from "./assets/icons/Discord.svg";
import MinecraftIcon from "./assets/icons/Minecraft.svg";
import RobloxIcon from "./assets/icons/Roblox.svg";
import SteamIcon from "./assets/icons/Steam.svg";
import VSCodeIcon from "./assets/icons/VS_code.svg";
import KritaIcon from "./assets/icons/Krita.svg";
import MusicIcon from "./assets/icons/Music.svg";

interface DesktopItem {
  name: string;
  path: string;
  is_dir: boolean;
}

interface ShortcutConfig {
  matchName: string; // Name of exact file/shortcut on Desktop
  icon: string; // Imported SVG icon
}

interface MediaState {
  is_active: boolean;
  title: string;
  artist: string;
  is_playing: boolean;
  thumbnail_base64?: string;
}

interface AgendaEvent {
  title: string;
  start_time: string;
  end_time: string;
  starts_in_ten: boolean;
  is_in_progress: boolean;
  is_all_day: boolean;
}

interface TodoItem {
  id: string;
  text: string;
  completed: boolean;
}

interface UserData {
  todos: TodoItem[];
  notes: string;
}

// Mapping of customized shortcut icons to desktop shortcuts
const SHORTCUT_CONFIG: ShortcutConfig[] = [
  { matchName: "Recycle Bin", icon: TrashIcon },
  { matchName: "2026 Fall Semester", icon: FolderIcon },
  { matchName: "calibre", icon: BooksIcon },
  { matchName: "Google Finance", icon: FinanceIcon },
  { matchName: "Zoom Workplace", icon: ZoomIcon },
  { matchName: "Discord", icon: DiscordIcon },
  { matchName: "Minecraft Launcher", icon: MinecraftIcon },
  { matchName: "Roblox Player", icon: RobloxIcon },
  { matchName: "Steam", icon: SteamIcon },
  { matchName: "Visual Studio Code", icon: VSCodeIcon },
  { matchName: "Krita", icon: KritaIcon },
  { matchName: "YouTube Music", icon: MusicIcon }
];

// --- Schedule Widget Component ---
function ScheduleWidget() {
  const [events, setEvents] = useState<AgendaEvent[]>([]);

  const fetchEvents = () => {
    invoke<AgendaEvent[]>("fetch_todays_events")
      .then((data) => setEvents(data))
      .catch((err) => console.error("Failed to fetch calendar agenda:", err));
  };

  useEffect(() => {
    fetchEvents();
    // Poll every 60 seconds to update time states, remove past events, and refresh countdowns
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

// --- Isolated Music Widget Component ---
function MusicWidget() {
  const [media, setMedia] = useState<MediaState | null>(null);

  useEffect(() => {
    // Restored to 1000ms since we no longer need fast CSS interpolation
    const interval = setInterval(async () => {
      try {
        const state = await invoke<MediaState>("get_media_state");
        setMedia(state);
      } catch (e) {
        console.error(e);
      }
    }, 1000);
    return () => clearInterval(interval);
  }, []);

  if (!media || !media.is_active || !media.title) return null;

  return (
    <div className="music-widget">
      {/* Conditionally render the album cover if Rust sends the Base64 string */}
      {media.thumbnail_base64 && (
        <img 
          src={`data:image/jpeg;base64,${media.thumbnail_base64}`} 
          alt="Album Art" 
          className="album-cover" 
        />
      )}
      
      <div className="music-info">
        <span className="music-title">{media.title}</span>
        <span className="music-artist">{media.artist}</span>
      </div>

      <div className="music-controls">
        <button 
          onPointerDown={() => invoke("play_ping", { soundType: "music" }).catch(console.error)} 
          onClick={() => invoke('media_prev')}
        >
          ⏮
        </button>

        <button 
          className="play-pause-btn" 
          onPointerDown={() => invoke("play_ping", { soundType: "music" }).catch(console.error)} 
          onClick={() => invoke('media_play_pause')}
        >
          {media.is_playing ? "⏸" : "▶"}
        </button>
        
        <button 
          onPointerDown={() => invoke("play_ping", { soundType: "music" }).catch(console.error)} 
          onClick={() => invoke('media_next')}
        >
          ⏭
        </button>
      </div>
    </div>
  );
}

// --- Isolated Notepad Widget Component ---
function NotepadWidget() {
  const [userData, setUserData] = useState<UserData>({ todos: [], notes: "" });
  const [activeTab, setActiveTab] = useState<"notes" | "todos">("notes");
  const [newTask, setNewTask] = useState("");
  const [isLoaded, setIsLoaded] = useState(false);
  
  // --- Pure React Drag and Drop State ---
  const [draggedIdx, setDraggedIdx] = useState<number | null>(null);
  const dragItemRef = useRef<number | null>(null);

  // 1. Load data on boot
  useEffect(() => {
    invoke<UserData>("load_user_data")
      .then((data) => {
        setUserData(data);
        setIsLoaded(true);
      })
      .catch(console.error);
  }, []);

  // 2. Debounced auto-save to prevent disk thrashing while typing
  useEffect(() => {
    if (!isLoaded) return;
    const timer = setTimeout(() => {
      invoke("save_user_data", { data: userData }).catch(console.error);
    }, 500);
    return () => clearTimeout(timer);
  }, [userData, isLoaded]);

  // 3. Global mouse release for drag and drop
  useEffect(() => {
    const handleGlobalUp = () => {
      dragItemRef.current = null;
      setDraggedIdx(null);
    };
    window.addEventListener("pointerup", handleGlobalUp);
    return () => window.removeEventListener("pointerup", handleGlobalUp);
  }, []);

  // --- Note Handlers ---
  const handleNoteChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setUserData((prev) => ({ ...prev, notes: e.target.value }));
  };

  // --- To-Do Handlers ---
  const handleAddTask = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && newTask.trim() !== "") {
      const newTodo: TodoItem = {
        id: crypto.randomUUID(),
        text: newTask.trim(),
        completed: false,
      };
      setUserData((prev) => ({ ...prev, todos: [...prev.todos, newTodo] }));
      setNewTask("");
    }
  };

  const toggleTodo = (id: string) => {
    setUserData((prev) => ({
      ...prev,
      todos: prev.todos.map((todo) =>
        todo.id === id ? { ...todo, completed: !todo.completed } : todo
      ),
    }));
  };

  const deleteTodo = (id: string) => {
    setUserData((prev) => ({
      ...prev,
      todos: prev.todos.filter((todo) => todo.id !== id),
    }));
  };

  // --- Pointer Drag Handlers ---
  const handlePointerDown = (e: React.PointerEvent, index: number) => {
    if ((e.target as HTMLElement).classList.contains("drag-handle")) {
      dragItemRef.current = index;
      setDraggedIdx(index);
      e.preventDefault();
    }
  };

  const handlePointerEnter = (targetIdx: number) => {
    const currentDrag = dragItemRef.current;
    if (currentDrag === null || currentDrag === targetIdx) return;

    setUserData((prev) => {
      const newTodos = [...prev.todos];
      const [movedTodo] = newTodos.splice(currentDrag, 1);
      newTodos.splice(targetIdx, 0, movedTodo);
      return { ...prev, todos: newTodos };
    });

    dragItemRef.current = targetIdx;
    setDraggedIdx(targetIdx);
  };

  return (
    <div className="notepad-widget">
      {/* Content Area */}
      <div className="notepad-content">
        {activeTab === "notes" ? (
          <textarea
            className="notes-textarea"
            placeholder="Jot down your thoughts..."
            value={userData.notes}
            onChange={handleNoteChange}
            spellCheck={false}
          />
        ) : (
          <div className="todo-container">
            <input
              type="text"
              className="todo-input"
              placeholder="+ Add a task (Press Enter)"
              value={newTask}
              onChange={(e) => setNewTask(e.target.value)}
              onKeyDown={handleAddTask}
            />
            <div className="todo-list">
              {userData.todos.map((todo, index) => (
                <div
                  key={todo.id}
                  className={`todo-item ${draggedIdx === index ? "dragging" : ""}`}
                  onPointerEnter={() => handlePointerEnter(index)}
                >
                  {/* Subtle Drag Handle Icon acts as the grip point */}
                  <span 
                    className="drag-handle"
                    onPointerDown={(e) => handlePointerDown(e, index)}
                  >
                    ⋮⋮
                  </span>
                  
                  <div className="keep-checkbox-wrapper">
                    <input
                      type="checkbox"
                      className="keep-checkbox"
                      checked={todo.completed}
                      onChange={() => toggleTodo(todo.id)}
                    />
                  </div>
                  <input
                    type="text"
                    className={`todo-text ${todo.completed ? "completed" : ""}`}
                    value={todo.text}
                    onChange={(e) => {
                      const newText = e.target.value;
                      setUserData((prev) => ({
                        ...prev,
                        todos: prev.todos.map((t) =>
                          t.id === todo.id ? { ...t, text: newText } : t
                        ),
                      }));
                    }}
                  />
                  <button className="todo-delete" onClick={() => deleteTodo(todo.id)}>
                    ✕
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Right-Side Tab Navigation */}
      <div className="notepad-tabs">
        <button
          className={`tab-button ${activeTab === "notes" ? "active" : ""}`}
          onClick={() => setActiveTab("notes")}
          title="Notes"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="16" y1="13" x2="8" y2="13" />
            <line x1="16" y1="17" x2="8" y2="17" />
            <polyline points="10 9 9 9 8 9" />
          </svg>
        </button>
        <button
          className={`tab-button ${activeTab === "todos" ? "active" : ""}`}
          onClick={() => setActiveTab("todos")}
          title="To-Do List"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="9 11 12 14 22 4" />
            <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
          </svg>
        </button>
      </div>
    </div>
  );
}

// --- Main App Canvas ---
export default function App() {
  const [items, setItems] = useState<DesktopItem[]>([]);
  const [currentTime, setCurrentTime] = useState(new Date());

  // Clock Tick Effect
  useEffect(() => {
    const timer = setInterval(() => {
      setCurrentTime(new Date());
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    // Read the user's desktop folder shortcuts from Rust
    invoke<DesktopItem[]>("get_desktop_items")
      .then((desktopFiles) => {
        // Match items by checking if matchName string is included in desktop file name
        const orderedItems = SHORTCUT_CONFIG.map((config) => {
          const found = desktopFiles.find(
            (f) => f.name.toLowerCase().includes(config.matchName.toLowerCase())
          );
          return (
            found || {
              name: config.matchName,
              path: "",
              is_dir: false,
            }
          );
        });
        setItems(orderedItems);
      })
      .catch(console.error);
  }, []);

  const handleClick = (name: string, path: string) => {
    // If it's the virtual Recycle Bin, use its Windows Shell URI
    const targetPath =
      name.toLowerCase() === "recycle bin" || name.toLowerCase() === "trash"
        ? "shell:RecycleBinFolder"
        : path;
    
    if (targetPath) {
      invoke("launch_item", { path: targetPath }).catch(console.error);
    }
  };

  // Format the time and date for display
  const fullTimeString = currentTime.toLocaleTimeString([], { 
    hour: '2-digit', 
    minute: '2-digit' 
  });
  const timeDigits = fullTimeString.replace(/\s?(AM|PM|am|pm)/i, '');
  const amPmMatch = fullTimeString.match(/(AM|PM|am|pm)/i);
  const amPmText = amPmMatch ? amPmMatch[0] : '';
  const formattedWeekday = currentTime.toLocaleDateString([], { weekday: 'long' });
  const formattedMonth = currentTime.toLocaleDateString([], { month: 'long' });
  const formattedDay = currentTime.toLocaleDateString([], { day: 'numeric' });

  return (
    <div
      className="desktop-canvas"
      style={{ backgroundImage: `url(${wallpaperImg})` }}
    >
      {/* Top Left: Time and Date Widget */}
      <div className="date-widget">
        <span className="date-weekday">{formattedWeekday},</span>
        <span className="date-month">{formattedMonth}</span>
        <span className="date-day">{formattedDay}</span>
      </div>

      <div className="time-widget">
        <span className="time-digits">{timeDigits}</span>
        <span className="time-ampm">{amPmText}</span>
      </div>

      {/* Schedule Widget (Right side under Date/Time/Music) */}
      <ScheduleWidget />

      {/* Notepad Widget */}
      <NotepadWidget/>
      
      {/* Custom Shortcut Grid */}
      <div className="shortcuts-grid"> 
        {SHORTCUT_CONFIG.map((config, index) => {
          const item = items[index];
          return (
            <button
              key={config.matchName}
              className="custom-shortcut"
              onPointerDown={() => invoke("play_ping", { soundType: "shortcut" }).catch(console.error)}
              onClick={() => handleClick(config.matchName, item?.path || "")}
              title={config.matchName}
            >
              <div className="icon-container">
                <img
                  src={config.icon}
                  alt={config.matchName}
                  className="custom-icon-img"
                />
              </div>
            </button>
          );
        })}
      </div>

      {/* Windows SMTC Music Player Widget */}
      <MusicWidget />
    </div>
  );
}