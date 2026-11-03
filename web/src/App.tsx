import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { NavLink, Route, Routes } from "react-router";
import { api } from "./api";
import { HomePage } from "./pages/Home";
import { LoginPage } from "./pages/Login";
import { SearchPage } from "./pages/Search";
import { SessionEditorPage } from "./pages/SessionEditor";
import { SessionsPage } from "./pages/Sessions";
import { usePlayer } from "./player/PlayerContext";
import { PlayerLayer } from "./player/PlayerLayer";
import { installBackHandler } from "./tv";

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
          Home
        </NavLink>
        <NavLink to="/search">Search</NavLink>
        <NavLink to="/multiview" end>
          Multiview
        </NavLink>
      </nav>
      <div className="header__user">
        {auth.data?.state === "logged_in" ? (
          <>
            <img className="avatar avatar--small" src={auth.data.user.profile_image_url} alt="" />
            <span>{auth.data.user.display_name}</span>
            <button className="btn btn--ghost btn--small" onClick={() => logout.mutate()}>
              Log out
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

/** The TV remote's Back key: the topmost thing closes first (modal, full-screen player). */
function BackKey() {
  const { target, mode, minimize } = usePlayer();
  useEffect(
    () =>
      installBackHandler(() => {
        if (document.querySelector(".modal")) {
          window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
          return true;
        }
        if (target && mode === "full") {
          minimize();
          return true;
        }
        return false;
      }),
    [target, mode, minimize],
  );
  return null;
}

export function App() {
  return (
    <>
      <BackKey />
      <Header />
      <main>
        <Routes>
          <Route path="/" element={<HomePage />} />
          <Route path="/search" element={<SearchPage />} />
          <Route path="/login" element={<LoginPage />} />
          <Route path="/multiview" element={<SessionsPage />} />
          <Route path="/multiview/:id" element={<SessionEditorPage />} />
          <Route path="*" element={<HomePage />} />
        </Routes>
      </main>
      {/* Outside <Routes>: navigation never unmounts the player. */}
      <PlayerLayer />
    </>
  );
}
