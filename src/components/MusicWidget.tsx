import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface MediaState {
  is_active: boolean;
  title: string;
  artist: string;
  is_playing: boolean;
  thumbnail_base64?: string;
}

export default function MusicWidget() {
  const [media, setMedia] = useState<MediaState | null>(null);
  const ignorePollUntil = useRef<number>(0);

  const fetchMediaState = async () => {
    if (Date.now() < ignorePollUntil.current) return;

    try {
      const state = await invoke<MediaState>("get_media_state");
      setMedia(state);
    } catch (e) {
      console.error(e);
    }
  };

  useEffect(() => {
    fetchMediaState();
    const interval = setInterval(fetchMediaState, 500);
    return () => clearInterval(interval);
  }, []);

  const handlePlayPause = async () => {
    invoke("play_ping", { soundType: "music" }).catch(console.error);
    
    if (media) {
      setMedia({ ...media, is_playing: !media.is_playing });
    }
    ignorePollUntil.current = Date.now() + 500;
    
    await invoke('media_play_pause');
  };

  const handleSkip = async (direction: 'media_next' | 'media_prev') => {
    invoke("play_ping", { soundType: "music" }).catch(console.error);
    
    if (media) {
      setMedia({ ...media, is_active: false });
    }
    
    ignorePollUntil.current = Date.now() + 800;
    await invoke(direction);
  };

  if (!media) return null;

  const hideWidget = !media.is_active || !media.title;

  return (
    <div className={`music-widget ${hideWidget ? "sliding-up" : ""}`}>
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