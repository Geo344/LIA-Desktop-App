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

interface TodoList {
  id: string;
  title: string;
  archived: boolean;
  items: TodoItem[];
}

interface UserData {
  lists: TodoList[];
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
  const [isSliding, setIsSliding] = useState(false);
  
  // Tracks the exact millisecond a button was last clicked
  const lastActionTime = useRef<number>(0);

  const fetchMediaState = async () => {
    try {
      const state = await invoke<MediaState>("get_media_state");
      
      // Only apply the fetched state if we haven't clicked a button in the last 800ms.
      if (Date.now() - lastActionTime.current > 800) {
        setMedia(state);
      }
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => {
    fetchMediaState();
    const interval = setInterval(fetchMediaState, 1000);
    return () => clearInterval(interval);
  }, []);

  // --- Highly Responsive Handlers ---
  const handlePlayPause = async () => {
    invoke("play_ping", { soundType: "music" }).catch(console.error);
    lastActionTime.current = Date.now();
    
    if (media) {
      setMedia({ ...media, is_playing: !media.is_playing });
    }
    
    await invoke('media_play_pause');
  };

  const handleSkip = async (direction: 'media_next' | 'media_prev') => {
    if (isSliding) return; // Prevent spam-clicking while animating
    
    invoke("play_ping", { soundType: "music" }).catch(console.error);
    lastActionTime.current = Date.now();
    
    // 1. Trigger the slide-up animation instantly
    setIsSliding(true);
    
    // 2. Send the command to Windows
    await invoke(direction);
    
    // 3. Wait 300ms for the upward animation to finish and SMTC to fetch the new track,
    // then fetch the fresh data and slide it back down.
    setTimeout(async () => {
      lastActionTime.current = 0;
      await fetchMediaState();
      setIsSliding(false);
    }, 300);
  };

  if (!media || !media.is_active || !media.title) return null;

  return (
    <div className={`music-widget ${isSliding ? "sliding-up" : ""}`}>
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
        <button onPointerDown={() => handleSkip('media_prev')}>
          <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
            <polygon points="19 20 9 12 19 4 19 20"></polygon>
            <rect x="5" y="4" width="2" height="16"></rect>
          </svg>
        </button>

        <button 
          className="play-pause-btn" 
          onPointerDown={handlePlayPause}
        >
          {media.is_playing ? (
            <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
              <rect x="6" y="4" width="4" height="16"></rect>
              <rect x="14" y="4" width="4" height="16"></rect>
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="currentColor" width="20" height="20">
              <polygon points="6 3 20 12 6 21 6 3"></polygon>
            </svg>
          )}
        </button>
        
        <button onPointerDown={() => handleSkip('media_next')}>
          <svg viewBox="0 0 24 24" fill="currentColor" width="16" height="16">
            <polygon points="5 4 15 12 5 20 5 4"></polygon>
            <rect x="17" y="4" width="2" height="16"></rect>
          </svg>
        </button>
      </div>
    </div>
  );
}

function NotepadWidget() {
  const [userData, setUserData] = useState<UserData>({ lists: [], notes: "" });
  const [activeTab, setActiveTab] = useState<"notes" | "todos">("notes");
  const [activeListId, setActiveListId] = useState<string | null>(null);
  
  // --- Archive & Visibility State ---
  const [showArchivedView, setShowArchivedView] = useState(false);
  const [archiveSearchQuery, setArchiveSearchQuery] = useState("");
  const [isHidden, setIsHidden] = useState(true); // Hidden by default
  
  const [isLoaded, setIsLoaded] = useState(false);
  
  // --- Pure React Drag and Drop State ---
  const [draggedIdx, setDraggedIdx] = useState<number | null>(null);
  const dragItemRef = useRef<number | null>(null);

  // --- Audio Helpers ---
  const playClick = () => invoke("play_ping", { soundType: "notepad_click" }).catch(console.error);

  const playClick2 = () => invoke("play_ping", { soundType: "notepad_check" }).catch(console.error);

  const handleTabClick = (targetTab: "notes" | "todos") => {
    if (isHidden) {
      invoke("play_ping", { soundType: "notepad_open" }).catch(console.error);
      setIsHidden(false);
      setActiveTab(targetTab);
    } else if (activeTab !== targetTab) {
      invoke("play_ping", { soundType: "notepad_switch" }).catch(console.error);
      setActiveTab(targetTab);
    } else {
      // Hides the widget when clicking the currently active tab
      playClick();
      setIsHidden(true);
    }
  };

  // 1. Load data on boot
  useEffect(() => {
    invoke<UserData>("load_user_data")
      .then((data) => {
        setUserData(data);
        setIsLoaded(true);
      })
      .catch(console.error);
  }, []);

  // 2. Debounced auto-save
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

  // --- List Directory Handlers ---
  const handleAddNewList = () => {
    const newList: TodoList = {
      id: crypto.randomUUID(),
      title: "",
      archived: false,
      items: [],
    };
    setUserData((prev) => ({ ...prev, lists: [...prev.lists, newList] }));
    setActiveListId(newList.id);
  };

  const handleTitleChange = (id: string, newTitle: string) => {
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) => (l.id === id ? { ...l, title: newTitle } : l)),
    }));
  };

  const toggleArchiveList = (id: string, e: React.MouseEvent) => {
    e.stopPropagation(); 
    playClick();
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) => (l.id === id ? { ...l, archived: !l.archived } : l)),
    }));
  };

  const deleteList = (id: string, e: React.MouseEvent) => {
    e.stopPropagation(); 
    playClick();
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.filter((l) => l.id !== id),
    }));
    if (activeListId === id) setActiveListId(null);
  };

  // --- To-Do Handlers ---
  const handleAddNewTask = () => {
    if (!activeListId) return;
    playClick2();
    const newTodo: TodoItem = {
      id: crypto.randomUUID(),
      text: "",
      completed: false,
    };
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) =>
        l.id === activeListId ? { ...l, items: [...l.items, newTodo] } : l
      ),
    }));
  };

  const toggleTodo = (todoId: string) => {
    playClick2();
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) =>
        l.id === activeListId
          ? {
              ...l,
              items: l.items.map((t) =>
                t.id === todoId ? { ...t, completed: !t.completed } : t
              ),
            }
          : l
      ),
    }));
  };

  const deleteTodo = (todoId: string) => {
    playClick();
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) =>
        l.id === activeListId
          ? { ...l, items: l.items.filter((t) => t.id !== todoId) }
          : l
      ),
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
    if (currentDrag === null || currentDrag === targetIdx || !activeListId) return;

    setUserData((prev) => {
      const newLists = [...prev.lists];
      const listIdx = newLists.findIndex((l) => l.id === activeListId);
      if (listIdx === -1) return prev;

      const newItems = [...newLists[listIdx].items];
      const [movedItem] = newItems.splice(currentDrag, 1);
      newItems.splice(targetIdx, 0, movedItem);

      newLists[listIdx] = { ...newLists[listIdx], items: newItems };
      return { ...prev, lists: newLists };
    });

    dragItemRef.current = targetIdx;
    setDraggedIdx(targetIdx);
  };

  // Split data for rendering
  const activeLists = userData.lists.filter((l) => !l.archived);
  const archivedLists = userData.lists
    .filter((l) => l.archived)
    .filter((l) => {
      const displayTitle = l.title || "Untitled";
      return displayTitle.toLowerCase().includes(archiveSearchQuery.toLowerCase());
    });

  const activeList = userData.lists.find((l) => l.id === activeListId);
  const activeTodos = activeList?.items.filter((t) => !t.completed) || [];
  const completedTodos = activeList?.items.filter((t) => t.completed) || [];

  return (
    <div className={`notepad-widget ${isHidden ? "hidden" : ""}`}>
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
            {/* DIRECTORY VIEWS */}
            {!activeListId ? (
              showArchivedView ? (
                /* 3rd View: ARCHIVED LIST DIRECTORY */
                <div className="directory-view">
                  <input
                    type="text"
                    className="archive-search-input"
                    placeholder="Search archives..."
                    value={archiveSearchQuery}
                    onChange={(e) => setArchiveSearchQuery(e.target.value)}
                  />
                  <div className="directory-list">
                    {archivedLists.map((list) => (
                      <div
                        key={list.id}
                        className="directory-item archived-row"
                        onClick={() => { playClick2(); setActiveListId(list.id); }}
                      >
                        <span className={`directory-title ${!list.title ? "untitled" : ""}`}>
                          {list.title || "Untitled"}
                        </span>
                        <div className="directory-actions">
                          <button
                            className="list-action-btn"
                            onClick={(e) => toggleArchiveList(list.id, e)}
                            title="Unarchive List"
                          >
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                              <polyline points="21 8 21 21 3 21 3 8"></polyline>
                              <rect x="1" y="3" width="22" height="5"></rect>
                              <line x1="12" y1="17" x2="12" y2="12"></line>
                              <polyline points="15 14 12 11 9 14"></polyline>
                            </svg>
                          </button>
                          <button
                            className="list-action-btn delete"
                            onClick={(e) => deleteList(list.id, e)}
                            title="Delete List"
                          >
                            ✕
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                  <button 
                    className="archive-nav-btn" 
                    onClick={() => {
                      playClick();
                      setShowArchivedView(false);
                      setArchiveSearchQuery("");
                    }}
                  >
                    Back
                  </button>
                </div>
              ) : (
                /* 1st View: MAIN LIST DIRECTORY */
                <div className="directory-view">
                  <button className="add-list-btn" onClick={() => { playClick2(); handleAddNewList(); }}>
                    + New List
                  </button>
                  <div className="directory-list">
                    {activeLists.map((list) => (
                      <div
                        key={list.id}
                        className="directory-item"
                        onClick={() => { playClick2(); setActiveListId(list.id); }}
                      >
                        <span className={`directory-title ${!list.title ? "untitled" : ""}`}>
                          {list.title || "Untitled"}
                        </span>
                        <div className="directory-actions">
                          <button
                            className="list-action-btn"
                            onClick={(e) => toggleArchiveList(list.id, e)}
                            title="Archive List"
                          >
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                              <polyline points="21 8 21 21 3 21 3 8"></polyline>
                              <rect x="1" y="3" width="22" height="5"></rect>
                              <line x1="10" y1="12" x2="14" y2="12"></line>
                            </svg>
                          </button>
                          <button
                            className="list-action-btn delete"
                            onClick={(e) => deleteList(list.id, e)}
                            title="Delete List"
                          >
                            ✕
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                  <button 
                    className="archive-nav-btn" 
                    onClick={() => { playClick(); setShowArchivedView(true); }}
                  >
                    Archived Lists
                  </button>
                </div>
              )
            ) : (
              /* 2nd View: ACTIVE LIST VIEW */
              <div className="active-list-view">
                <div className="active-list-header">
                  <input
                    type="text"
                    className={`list-title-input ${!activeList?.title ? "untitled" : ""}`}
                    placeholder="Untitled"
                    value={activeList?.title || ""}
                    onChange={(e) => handleTitleChange(activeListId, e.target.value)}
                  />
                  <button className="back-btn" onClick={() => { playClick(); setActiveListId(null); }} title="Back to Lists">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <polyline points="15 18 9 12 15 6" />
                    </svg>
                  </button>
                </div>

                <div className="todo-list">
                  {/* Active Tasks (Draggable) */}
                  {activeTodos.map((todo) => {
                    const globalIdx = activeList!.items.findIndex((t) => t.id === todo.id);
                    return (
                      <div
                        key={todo.id}
                        className={`todo-item ${draggedIdx === globalIdx ? "dragging" : ""}`}
                        onPointerEnter={() => handlePointerEnter(globalIdx)}
                      >
                        <span 
                          className="drag-handle"
                          onPointerDown={(e) => handlePointerDown(e, globalIdx)}
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
                          className="todo-text"
                          placeholder="Empty task..."
                          value={todo.text}
                          onChange={(e) => {
                            const newText = e.target.value;
                            setUserData((prev) => ({
                              ...prev,
                              lists: prev.lists.map((l) =>
                                l.id === activeListId
                                  ? { ...l, items: l.items.map((t) => t.id === todo.id ? { ...t, text: newText } : t) }
                                  : l
                              ),
                            }));
                          }}
                        />
                        <button className="todo-delete" onClick={() => deleteTodo(todo.id)}>✕</button>
                      </div>
                    );
                  })}

                  <button className="add-item-btn" onClick={handleAddNewTask}>+ Item</button>

                  {/* Completed Tasks */}
                  <div className="completed-section">
                    <div className="completed-header">{completedTodos.length} Completed</div>
                    {completedTodos.map((todo) => (
                      <div key={todo.id} className="todo-item completed-row">
                        <span className="drag-handle invisible-handle">⋮⋮</span>
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
                          className="todo-text completed"
                          value={todo.text}
                          onChange={(e) => {
                            const newText = e.target.value;
                            setUserData((prev) => ({
                              ...prev,
                              lists: prev.lists.map((l) =>
                                l.id === activeListId
                                  ? { ...l, items: l.items.map((t) => t.id === todo.id ? { ...t, text: newText } : t) }
                                  : l
                              ),
                            }));
                          }}
                        />
                        <button className="todo-delete" onClick={() => deleteTodo(todo.id)}>✕</button>
                      </div>
                    ))}
                  </div>

                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Right-Side Tab Navigation */}
      <div className="notepad-tabs">
        <button
          className={`tab-button ${activeTab === "notes" ? "active" : ""}`}
          onClick={() => handleTabClick("notes")}
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
          onClick={() => handleTabClick("todos")}
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