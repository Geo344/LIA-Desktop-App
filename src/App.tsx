import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import ScheduleWidget from "./components/ScheduleWidget";
import MusicWidget from "./components/MusicWidget";
import NotepadWidget from "./components/NotepadWidget";

import "./App.css";
import "./css-styling/shortcuts.css";
import "./css-styling/music_widget.css";
import "./css-styling/notepad_widget.css";
import "./css-styling/schedule_widget.css";
import wallpaperImg from "./assets/Vivy_Wallpaper.png";

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

export interface DesktopItem {
  name: string;
  path: string;
  is_dir: boolean;
}

export interface ShortcutConfig {
  matchName: string;
  icon: string;
}

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

export default function App() {
  const [items, setItems] = useState<DesktopItem[]>([]);
  const [currentTime, setCurrentTime] = useState(new Date());

  useEffect(() => {
    const timer = setInterval(() => setCurrentTime(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    invoke<DesktopItem[]>("get_desktop_items")
      .then((desktopFiles) => {
        const orderedItems = SHORTCUT_CONFIG.map((config) => {
          const found = desktopFiles.find(
            (f) => f.name.toLowerCase().includes(config.matchName.toLowerCase())
          );
          return found || { name: config.matchName, path: "", is_dir: false };
        });
        setItems(orderedItems);
      })
      .catch(console.error);
  }, []);

  const handleClick = (name: string, path: string) => {
    const targetPath =
      name.toLowerCase() === "recycle bin" || name.toLowerCase() === "trash"
        ? "shell:RecycleBinFolder"
        : path;
    
    if (targetPath) {
      invoke("launch_item", { path: targetPath }).catch(console.error);
    }
  };

  const fullTimeString = currentTime.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const timeDigits = fullTimeString.replace(/\s?(AM|PM|am|pm)/i, '');
  const amPmMatch = fullTimeString.match(/(AM|PM|am|pm)/i);
  const amPmText = amPmMatch ? amPmMatch[0] : '';
  const formattedWeekday = currentTime.toLocaleDateString([], { weekday: 'long' });
  const formattedMonth = currentTime.toLocaleDateString([], { month: 'long' });
  const formattedDay = currentTime.toLocaleDateString([], { day: 'numeric' });

  return (
    <div className="desktop-canvas" style={{ backgroundImage: `url(${wallpaperImg})` }}>
      <div className="date-widget">
        <span className="date-weekday">{formattedWeekday},</span>
        <span className="date-month">{formattedMonth}</span>
        <span className="date-day">{formattedDay}</span>
      </div>

      <div className="time-widget">
        <span className="time-digits">{timeDigits}</span>
        <span className="time-ampm">{amPmText}</span>
      </div>

      <ScheduleWidget />
      <NotepadWidget />
      
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
                <img src={config.icon} alt={config.matchName} className="custom-icon-img" />
              </div>
            </button>
          );
        })}
      </div>

      <MusicWidget />
    </div>
  );
}