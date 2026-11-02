import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import { api } from "../api";
import { StreamCard } from "../components/StreamCard";

export function HomePage() {
  const auth = useQuery({ queryKey: ["auth"], queryFn: api.auth });
  const loggedIn = auth.data?.state === "logged_in";
  const follows = useQuery({
    queryKey: ["follows"],
    queryFn: api.followsLive,
    enabled: loggedIn,
    refetchInterval: 60_000,
  });

  if (auth.isPending) return <div className="page muted">Loading…</div>;

  if (!loggedIn) {
    return (
      <div className="page page--center">
        <div className="empty">
          <h2>See who's live</h2>
          <p className="muted">Log in to see the channels you follow. You can also just search for a channel.</p>
          <div className="row">
            <Link className="btn" to="/login">
              Log in with Twitch
            </Link>
            <Link className="btn btn--ghost" to="/search">
              Search
            </Link>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="page">
      <h2>Live now</h2>
      {follows.isPending && <p className="muted">Loading…</p>}
      {follows.isError && <p className="error">Could not load followed channels: {follows.error.message}</p>}
      {follows.data?.length === 0 && <p className="muted">None of the channels you follow are live right now.</p>}
      <div className="grid">
        {follows.data?.map((s) => (
          <StreamCard key={s.login} stream={s} />
        ))}
      </div>
    </div>
  );
}
