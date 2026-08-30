import { useEffect, useState } from "react";
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

// Mapping of customized shortcut icons to desktop shortcuts
const SHORTCUT_CONFIG: ShortcutConfig[] = [
  // Column 1
  { matchName: "Recycle Bin", icon: TrashIcon },
  { matchName: "2026 Fall Semester", icon: FolderIcon },
  { matchName: "calibre", icon: BooksIcon },
  { matchName: "Google Finance", icon: FinanceIcon },
  { matchName: "Zoom Workplace", icon: ZoomIcon },
  { matchName: "Discord", icon: DiscordIcon },
  
  // Column 2
  { matchName: "Minecraft Launcher", icon: MinecraftIcon },
  { matchName: "Roblox Player", icon: RobloxIcon },
  { matchName: "Steam", icon: SteamIcon },
  { matchName: "Visual Studio Code", icon: VSCodeIcon },
  { matchName: "Krita", icon: KritaIcon },
  { matchName: "YouTube Music", icon: MusicIcon }
];

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