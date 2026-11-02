// Device code login (D5): the screen shows a QR code and a short code; you confirm on your phone.
// The server polls Twitch, so this page only has to watch /api/auth.

import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { useNavigate } from "react-router";
import { api } from "../api";

function useCountdown(until: number | undefined): string {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (!until) return;
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, [until]);
  if (!until) return "";
  const left = Math.max(0, Math.round(until - now));
  return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
}

export function LoginPage() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const auth = useQuery({
    queryKey: ["auth"],
    queryFn: api.auth,
    refetchInterval: (q) => (q.state.data?.state === "pending" ? 2000 : false),
  });
  const start = useMutation({
    mutationFn: api.startLogin,
    onSuccess: (status) => qc.setQueryData(["auth"], status),
  });

  const state = auth.data?.state;
  // Start right away: on a TV, every extra button press counts.
  useEffect(() => {
    if (state === "logged_out" && !start.isPending && !start.isError) start.mutate();
  }, [state, start]);

  useEffect(() => {
    if (state === "logged_in") {
      qc.invalidateQueries({ queryKey: ["follows"] });
      navigate("/", { replace: true });
    }
  }, [state, qc, navigate]);

  const pending = auth.data?.state === "pending" ? auth.data : null;
  const countdown = useCountdown(pending?.expires_at);

  return (
    <div className="page page--center">
      <div className="login">
        <h1>Log in with Twitch</h1>
        {pending ? (
          <>
            <p className="muted">
              Scan the code with your phone, or open the link on any device.
              <br />
              Log in, then press <strong>Authorize</strong> and wait for "Success".
            </p>
            <div className="login__qr">
              <QRCodeSVG value={pending.verification_uri} size={220} marginSize={2} />
            </div>
            <a className="login__link" href={pending.verification_uri} target="_blank" rel="noreferrer">
              twitch.tv/activate
            </a>
            <p>
              Code: <span className="login__code">{pending.user_code}</span>
            </p>
            <p className="muted">Waiting for confirmation… expires in {countdown}</p>
          </>
        ) : start.isError ? (
          <>
            <p className="error">Could not start the login: {start.error.message}</p>
            <button className="btn" onClick={() => start.reset()}>
              Try again
            </button>
          </>
        ) : (
          <p className="muted">Preparing login…</p>
        )}
      </div>
    </div>
  );
}
