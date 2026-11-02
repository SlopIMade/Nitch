import { formatViewers, type LiveStream } from "../api";
import { usePlayer } from "../player/PlayerContext";

export function StreamCard({ stream }: { stream: LiveStream }) {
  const { play } = usePlayer();
  return (
    <button className="card" onClick={() => play(stream.login)} title={stream.title}>
      <div className="card__thumb">
        <img src={stream.thumbnail_url} alt="" loading="lazy" />
        <span className="badge badge--live">LIVE</span>
        <span className="badge badge--viewers">{formatViewers(stream.viewers)}</span>
      </div>
      <div className="card__body">
        <div className="card__title">{stream.title}</div>
        <div className="card__meta">
          <strong>{stream.display_name}</strong> · {stream.game}
        </div>
      </div>
    </button>
  );
}
