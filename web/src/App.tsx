import { useEffect, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate, NavLink, Route, Routes } from "react-router";
import { api } from "./api";
import { Icon } from "./components/Icon";
import { HomePage } from "./pages/Home";
import { LoginPage } from "./pages/Login";
import { SearchPage } from "./pages/Search";
import { SessionEditorPage } from "./pages/SessionEditor";
import { SettingsPage } from "./pages/Settings";
import { usePlayer } from "./player/PlayerContext";
import { PlayerLayer } from "./player/PlayerLayer";
import { installBackHandler } from "./tv";
import { watchEvents } from "./player/chat";

function Header() {
  const qc = useQueryClient();
  const auth = useQuery({ queryKey: ["auth"], queryFn: api.auth });
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: (status) => {
      qc.setQueryData(["auth"], status);
      qc.removeQueries({ queryKey: ["follows"] });
    },
  });

  return (
    <header className="header">
      <span className="logo">Nitch</span>
      <nav className="nav">
        <NavLink to="/" end>
          <Icon name="layout" /> Home
        </NavLink>
        <NavLink to="/search">
          <Icon name="search" /> Search
        </NavLink>
        <NavLink to="/settings">
          <Icon name="settings" /> Settings
        </NavLink>
      </nav>
      <div className="header__user">
        {auth.data?.state === "logged_in" ? (
          <>
            <img className="avatar avatar--small" src={auth.data.user.profile_image_url} alt="" />
            <span>{auth.data.user.display_name}</span>
            <button className="btn btn--ghost btn--small" onClick={() => logout.mutate()}>
              <Icon name="logout" /> Log out
            </button>
          </>
        ) : (
          <NavLink className="btn btn--small" to="/login">
            Log in
          </NavLink>
        )}
      </div>
    </header>
  );
}

/**
 * The TV remote's Back key: the topmost thing closes first. The player's side menu and chat
 * adjusting handle Back themselves; then a modal, then the player (playback ends).
 */
function BackKey() {
  const player = usePlayer();
  // Installed once, so it stays below the handlers of menus that open later.
  const current = useRef(player);
  current.current = player;
  useEffect(
    () =>
      installBackHandler(() => {
        if (document.querySelector(".modal")) {
          window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
          return true;
        }
        if (current.current.target) {
          current.current.stop();
          return true;
        }
        return false;
      }),
    [],
  );
  return null;
}

/** Server events: when any device edits a session, every device showing it follows. */
function ServerEvents() {
  const qc = useQueryClient();
  useEffect(
    () =>
      watchEvents((e) => {
        if (e.type === "session_updated") {
          qc.invalidateQueries({ queryKey: ["sessions"] });
          qc.invalidateQueries({ queryKey: ["session", e.id] });
          qc.invalidateQueries({ queryKey: ["multiview", e.id] });
        }
      }),
    [qc],
  );
  return null;
}

export function App() {
  return (
    <>
      <BackKey />
      <ServerEvents />
      <Header />
      <main>
        <Routes>
          <Route path="/" element={<HomePage />} />
          <Route path="/search" element={<SearchPage />} />
          <Route path="/login" element={<LoginPage />} />
          <Route path="/multiview" element={<Navigate to="/" replace />} />
          <Route path="/multiview/:id" element={<SessionEditorPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="*" element={<HomePage />} />
        </Routes>
      </main>
      {/* Outside <Routes>: navigation never unmounts the player. */}
      <PlayerLayer />
    </>
  );
}
