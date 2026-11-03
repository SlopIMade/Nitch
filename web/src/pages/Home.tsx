// The start page: saved multiview sessions (the same on every device, D17) and the followed
// channels that are live. A session plays with one click; a channel too, and while it plays,
// more streams are added from the player's side menu. Layouts are edited in the player.

import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { api, type Session } from "../api";
import { Icon } from "../components/Icon";
import { LayoutPreview } from "../components/LayoutPreview";
import { StreamCard } from "../components/StreamCard";
import { usePlayer } from "../player/PlayerContext";

/** Delete in two steps (no confirm(): the TV's WebView has no dialogs). */
function DeleteButton({ onDelete }: { onDelete: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button className={`btn btn--small ${armed ? "btn--danger" : "btn--ghost"}`} onClick={() => (armed ? onDelete() : setArmed(true))}>
      <Icon name="trash" /> {armed ? "Really delete?" : "Delete"}
    </button>
  );
}

function SessionCard({ s, onDelete }: { s: Session; onDelete: () => void }) {
  const { playSession } = usePlayer();
  return (
    <div className="card card--static">
      <button className="card__thumb card__thumb--layout" onClick={() => playSession(s.id, s.name)} title="Play">
        <LayoutPreview rects={s.tiles} labels={s.tiles.map((t) => t.channel)} />
      </button>
      <div className="card__body">
        <div className="card__title">{s.name}</div>
        <div className="row row--left">
          <DeleteButton onDelete={onDelete} />
        </div>
      </div>
    </div>
  );
}

export function HomePage() {
  const qc = useQueryClient();
  const auth = useQuery({ queryKey: ["auth"], queryFn: api.auth });
  const loggedIn = auth.data?.state === "logged_in";
  const follows = useQuery({ queryKey: ["follows"], queryFn: api.followsLive, enabled: loggedIn, refetchInterval: 60_000 });
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: api.sessions });
  const remove = useMutation({
    mutationFn: api.deleteSession,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sessions"] }),
  });

  return (
    <div className="page">
      {(sessions.data?.length ?? 0) > 0 && (
        <section className="home__section">
          <h2>Multiview</h2>
          <div className="grid grid--small">
            {sessions.data?.map((s) => (
              <SessionCard key={s.id} s={s} onDelete={() => remove.mutate(s.id)} />
            ))}
          </div>
        </section>
      )}
      {sessions.isError && <p className="error">Could not load sessions: {sessions.error.message}</p>}

      <section className="home__section">
        <h2>Live now</h2>
        {auth.isPending && <p className="muted">Loading…</p>}
        {auth.data && !loggedIn && (
          <p className="muted">
            <Link to="/login">Log in</Link> to see the channels you follow, or <Link to="/search">search</Link> for one.
          </p>
        )}
        {follows.isPending && loggedIn && <p className="muted">Loading…</p>}
        {follows.isError && <p className="error">Could not load followed channels: {follows.error.message}</p>}
        {follows.data?.length === 0 && <p className="muted">None of the channels you follow are live right now.</p>}
        <div className="grid">
          {follows.data?.map((s) => (
            <StreamCard key={s.login} stream={s} />
          ))}
        </div>
        {sessions.data?.length === 0 && (
          <p className="muted home__tip">
            Tip: while a stream plays, hold ← on the remote (or use “Streams”) to add more streams to the screen.
          </p>
        )}
      </section>
    </div>
  );
}
