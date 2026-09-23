import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface TodoItem {
  id: string;
  text: string;
  completed: boolean;
}

export interface TodoList {
  id: string;
  title: string;
  archived: boolean;
  items: TodoItem[];
}

export interface UserData {
  lists: TodoList[];
  notes: string;
}

export default function NotepadWidget() {
  const [userData, setUserData] = useState<UserData>({ lists: [], notes: "" });
  const [activeTab, setActiveTab] = useState<"notes" | "todos">("notes");
  const [activeListId, setActiveListId] = useState<string | null>(null);
  
  const [animatingTasks, setAnimatingTasks] = useState<string[]>([]);
  const [showArchivedView, setShowArchivedView] = useState(false);
  const [archiveSearchQuery, setArchiveSearchQuery] = useState("");
  const [isHidden, setIsHidden] = useState(true);
  const [isLoaded, setIsLoaded] = useState(false);
  
  const [draggedIdx, setDraggedIdx] = useState<number | null>(null);
  const dragItemRef = useRef<number | null>(null);
  const scrollListRef = useRef<HTMLDivElement>(null);

  // --- Audio Handlers ---
  const playClick = () => invoke("play_ping", { soundType: "notepad_click" }).catch(console.error);
  const playClick2 = () => invoke("play_ping", { soundType: "notepad_check" }).catch(console.error);
  const playClick3 = () => invoke("play_ping", { soundType: "notepad_switch" }).catch(console.error);

  // --- Lifecycle & Side Effects ---
  useEffect(() => {
    invoke<UserData>("load_user_data")
      .then((data) => {
        setUserData(data);
        setIsLoaded(true);
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    if (!isLoaded) return;
    const timer = setTimeout(() => {
      invoke("save_user_data", { data: userData }).catch(console.error);
    }, 500);
    return () => clearTimeout(timer);
  }, [userData, isLoaded]);

  useEffect(() => {
    const handleGlobalUp = () => {
      dragItemRef.current = null;
      setDraggedIdx(null);
    };
    window.addEventListener("pointerup", handleGlobalUp);
    return () => window.removeEventListener("pointerup", handleGlobalUp);
  }, []);

  useEffect(() => {
    if (draggedIdx === null) return;
    let animationFrameId: number;
    let currentScrollSpeed = 0;

    const handlePointerMove = (e: PointerEvent) => {
      if (!scrollListRef.current) return;
      const rect = scrollListRef.current.getBoundingClientRect();
      const threshold = 10;
      
      if (e.clientY < rect.top + threshold) currentScrollSpeed = -4;
      else if (e.clientY > rect.bottom - threshold) currentScrollSpeed = 4;
      else currentScrollSpeed = 0;
    };

    const scrollLoop = () => {
      if (currentScrollSpeed !== 0 && scrollListRef.current) {
        scrollListRef.current.scrollTop += currentScrollSpeed;
      }
      animationFrameId = requestAnimationFrame(scrollLoop);
    };

    window.addEventListener("pointermove", handlePointerMove);
    animationFrameId = requestAnimationFrame(scrollLoop);

    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      cancelAnimationFrame(animationFrameId);
    };
  }, [draggedIdx]);

  // --- UI Handlers ---
  const handleTabClick = (targetTab: "notes" | "todos") => {
    if (isHidden) {
      invoke("play_ping", { soundType: "notepad_open" }).catch(console.error);
      setIsHidden(false);
      setActiveTab(targetTab);
    } else if (activeTab !== targetTab) {
      playClick3();
      setActiveTab(targetTab);
    } else {
      playClick();
      setIsHidden(true);
    }
  };

  const handleNoteChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setUserData((prev) => ({ ...prev, notes: e.target.value }));
  };

  // --- List Data Handlers ---
  const handleAddNewList = () => {
    playClick2();
    const newList: TodoList = { id: crypto.randomUUID(), title: "", archived: false, items: [] };
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

  const copyList = (id: string, e: React.MouseEvent) => {
    e.stopPropagation(); 
    playClick();
    setUserData((prev) => {
      const listIndex = prev.lists.findIndex((l) => l.id === id);
      if (listIndex === -1) return prev;
      
      const listToCopy = prev.lists[listIndex];
      const copiedList: TodoList = {
        id: crypto.randomUUID(),
        title: listToCopy.title ? `${listToCopy.title} - copy` : "Untitled - copy",
        archived: listToCopy.archived,
        items: listToCopy.items.map(item => ({ ...item, id: crypto.randomUUID() }))
      };
      
      const newLists = [...prev.lists];
      newLists.splice(listIndex + 1, 0, copiedList);
      return { ...prev, lists: newLists };
    });
  };

  // --- Task Data Handlers ---
  const handleAddNewTask = () => {
    if (!activeListId) return;
    playClick2();
    const newTodo: TodoItem = { id: crypto.randomUUID(), text: "", completed: false };
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) => l.id === activeListId ? { ...l, items: [...l.items, newTodo] } : l),
    }));
  };

  const handleInsertTask = (e: React.KeyboardEvent<HTMLInputElement>, currentGlobalIdx: number) => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (!activeListId) return;
      playClick2();
      
      const newTodo: TodoItem = { id: crypto.randomUUID(), text: "", completed: false };
      setUserData((prev) => {
        const newLists = [...prev.lists];
        const listIdx = newLists.findIndex((l) => l.id === activeListId);
        if (listIdx === -1) return prev;
        
        const newItems = [...newLists[listIdx].items];
        newItems.splice(currentGlobalIdx + 1, 0, newTodo);
        newLists[listIdx] = { ...newLists[listIdx], items: newItems };
        return { ...prev, lists: newLists };
      });
      setTimeout(() => document.getElementById(`input-${newTodo.id}`)?.focus(), 10);
    }
  };

  const handleUpdateTodoText = (todoId: string, newText: string) => {
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) =>
        l.id === activeListId 
          ? { ...l, items: l.items.map((t) => t.id === todoId ? { ...t, text: newText } : t) } 
          : l
      ),
    }));
  };

  const toggleTodo = (todoId: string) => {
    playClick2();
    setAnimatingTasks((prev) => [...prev, todoId]);
    setTimeout(() => {
      setAnimatingTasks((prev) => prev.filter(id => id !== todoId));
      setUserData((prev) => ({
        ...prev,
        lists: prev.lists.map((l) =>
          l.id === activeListId
            ? { ...l, items: l.items.map((t) => t.id === todoId ? { ...t, completed: !t.completed } : t) }
            : l
        ),
      }));
    }, 200);
  };

  const deleteTodo = (todoId: string) => {
    playClick();
    setUserData((prev) => ({
      ...prev,
      lists: prev.lists.map((l) =>
        l.id === activeListId ? { ...l, items: l.items.filter((t) => t.id !== todoId) } : l
      ),
    }));
  };

  // --- Drag & Drop Handlers ---
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

  // --- Data Selectors ---
  const activeLists = userData.lists.filter((l) => !l.archived);
  const archivedLists = userData.lists
    .filter((l) => l.archived)
    .filter((l) => (l.title || "Untitled").toLowerCase().includes(archiveSearchQuery.toLowerCase()));
  
  const activeList = userData.lists.find((l) => l.id === activeListId);
  const activeTodos = activeList?.items.filter((t) => !t.completed) || [];
  const completedTodos = activeList?.items.filter((t) => t.completed) || [];

  // --- Reusable UI Renderers ---
  const renderListRow = (list: TodoList) => (
    <div key={list.id} className={`directory-item ${list.archived ? "archived-row" : ""}`} onClick={() => { playClick3(); setActiveListId(list.id); }}>
      <div className="directory-info">
        <span className={`directory-title ${!list.title ? "untitled" : ""}`}>{list.title || "Untitled"}</span>
        <div className="list-counts">
          <span className="count-pending" title="Remaining">{list.items.filter(t => !t.completed).length}</span>
          <span className="count-completed" title="Completed">{list.items.filter(t => t.completed).length}</span>
        </div>
      </div>
      <div className="directory-actions">
        <button className="list-action-btn" onClick={(e) => copyList(list.id, e)} title="Copy List">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
          </svg>
        </button>
        <button className="list-action-btn" onClick={(e) => toggleArchiveList(list.id, e)} title={list.archived ? "Unarchive List" : "Archive List"}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="21 8 21 21 3 21 3 8"></polyline>
            <rect x="1" y="3" width="22" height="5"></rect>
            <line x1="12" y1="17" x2="12" y2="12"></line>
            {list.archived ? <polyline points="15 14 12 11 9 14"></polyline> : <line x1="10" y1="12" x2="14" y2="12"></line>}
          </svg>
        </button>
        <button className="list-action-btn delete" onClick={(e) => deleteList(list.id, e)} title="Delete List">✕</button>
      </div>
    </div>
  );

  const renderTaskRow = (todo: TodoItem, globalIdx?: number) => {
    const isCompleted = todo.completed;
    const isDragging = draggedIdx === globalIdx && !isCompleted;

    return (
      <div 
        key={todo.id} 
        className={`todo-item ${isCompleted ? "completed-row" : ""} ${isDragging ? "dragging" : ""}`}
        onPointerEnter={!isCompleted && globalIdx !== undefined ? () => handlePointerEnter(globalIdx) : undefined}
      >
        <span 
          className={`drag-handle ${isCompleted ? "invisible-handle" : ""}`} 
          onPointerDown={!isCompleted && globalIdx !== undefined ? (e) => handlePointerDown(e, globalIdx) : undefined}
        >
          ⋮⋮
        </span>
        <div className="keep-checkbox-wrapper">
          <input
            type="checkbox"
            className={`keep-checkbox ${animatingTasks.includes(todo.id) ? "pop-animate" : ""}`}
            checked={isCompleted}
            onChange={() => toggleTodo(todo.id)}
          />
        </div>
        <input
          id={!isCompleted ? `input-${todo.id}` : undefined}
          type="text"
          className={`todo-text ${isCompleted ? "completed" : ""}`}
          placeholder={!isCompleted ? "Empty task..." : ""}
          value={todo.text}
          autoComplete="off" /* Add this line to block Edge autofill */
          onKeyDown={!isCompleted && globalIdx !== undefined ? (e) => handleInsertTask(e, globalIdx) : undefined}
          onChange={(e) => handleUpdateTodoText(todo.id, e.target.value)}
        />
        <button className="todo-delete" onClick={() => deleteTodo(todo.id)}>✕</button>
      </div>
    );
  };

  const renderDirectoryView = () => {
    if (showArchivedView) {
      return (
        <div className="directory-view">
          <input
            type="text"
            className="archive-search-input"
            placeholder="Search archives..."
            value={archiveSearchQuery}
            autoComplete="off" /* Add this line to block Edge autofill */
            onChange={(e) => setArchiveSearchQuery(e.target.value)}
          />
          <div className="directory-list">
            {archivedLists.map(renderListRow)}
          </div>
          <button className="archive-nav-btn" onClick={() => { playClick(); setShowArchivedView(false); setArchiveSearchQuery(""); }}>Back</button>
        </div>
      );
    }

    return (
      <div className="directory-view">
        <button className="add-list-btn" onClick={handleAddNewList}>+ New List</button>
        <div className="directory-list">
          {activeLists.map(renderListRow)}
        </div>
        <button className="archive-nav-btn" onClick={() => { playClick(); setShowArchivedView(true); }}>To Archive</button>
      </div>
    );
  };

  return (
    <div className={`notepad-widget ${isHidden ? "hidden" : ""}`}>
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
            {!activeListId ? (
              renderDirectoryView()
            ) : (
              <div className="active-list-view">
                <div className="active-list-header">
                  <div className="directory-info">
                    <input
                      type="text"
                      className={`list-title-input ${!activeList?.title ? "untitled" : ""}`}
                      placeholder="Untitled"
                      value={activeList?.title || ""}
                      autoComplete="off" /* Add this line to block Edge autofill */
                      onChange={(e) => handleTitleChange(activeListId, e.target.value)}
                    />
                    <div className="list-counts" style={{ marginRight: "12px" }}>
                      <span className="count-pending" title="Remaining">{activeTodos.length}</span>
                      <span className="count-completed" title="Completed">{completedTodos.length}</span>
                    </div>
                  </div>
                  <button className="back-btn" onClick={() => { playClick(); setActiveListId(null); }} title="Back to Lists">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                      <polyline points="15 18 9 12 15 6" />
                    </svg>
                  </button>
                </div>

                <div className="todo-list" ref={scrollListRef}>
                  {activeTodos.map((todo) => {
                    const globalIdx = activeList!.items.findIndex((t) => t.id === todo.id);
                    return renderTaskRow(todo, globalIdx);
                  })}
                  
                  <button className="add-item-btn" onClick={handleAddNewTask}>+ Item</button>

                  <div className="completed-section">
                    <div className="completed-header">{completedTodos.length} Completed</div>
                    {completedTodos.map(todo => renderTaskRow(todo))}
                  </div>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="notepad-tabs">
        <button className={`tab-button ${activeTab === "notes" ? "active" : ""}`} onClick={() => handleTabClick("notes")} title="Notes">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="16" y1="13" x2="8" y2="13" />
            <line x1="16" y1="17" x2="8" y2="17" />
            <polyline points="10 9 9 9 8 9" />
          </svg>
        </button>
        <button className={`tab-button ${activeTab === "todos" ? "active" : ""}`} onClick={() => handleTabClick("todos")} title="To-Do List">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="9 11 12 14 22 4" />
            <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
          </svg>
        </button>
      </div>
    </div>
  );
}