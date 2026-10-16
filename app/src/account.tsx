// The Discord account of Klip, shared by the account button in the HUD bar, the question
// after the first start and the cloud settings. Signing in links this PC to Klip Cloud
// through a short code that is confirmed in the browser; the token stays in the backend.

import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { call, errorText } from "./lib/api";
import { t } from "./lib/i18n";
import { useApp } from "./state";
import { useToast } from "./ui";

export const DEFAULT_CLOUD_SERVER = "https://imkirit.dev/klip/cloud";

export interface CloudAccount {
  server: string;
  name: string;
  avatar: string | null;
}

interface AccountState {
  /** Null while signed out (or signed in to another server than the one in the settings). */
  account: CloudAccount | null;
  /** The code to confirm in the browser while a sign in runs. */
  linking: string | null;
  /** The page where the code is confirmed, to open it again by hand. */
  linkUrl: string | null;
  signIn: () => Promise<void>;
  signOut: () => Promise<void>;
  reload: () => Promise<void>;
}

const Context = createContext<AccountState | null>(null);

export function useAccount(): AccountState {
  const state = useContext(Context);
  if (!state) throw new Error("AccountState is missing");
  return state;
}

const clean = (url: string) => url.replace(/\/+$/, "");

export function AccountProvider(props: { children: ReactNode }) {
  const { config } = useApp();
  const toast = useToast();
  const server = clean(config.cloud?.server || DEFAULT_CLOUD_SERVER);
  const [stored, setStored] = useState<CloudAccount | null>(null);
  const [linking, setLinking] = useState<string | null>(null);
  const [linkUrl, setLinkUrl] = useState<string | null>(null);
  const running = useRef(false);

  const reload = useCallback(async () => {
    try {
      const r = await call<{ account: CloudAccount | null }>("cloud_account");
      setStored(r.account);
    } catch {
      setStored(null);
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const signIn = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    try {
      const link = await call<{ device_code: string; user_code: string; verify_url: string; interval: number; expires_in: number }>("cloud_link_start", { server });
      setLinking(link.user_code);
      setLinkUrl(link.verify_url);
      await call("open_url", { url: link.verify_url }).catch(() => window.open(link.verify_url, "_blank"));
      const until = Date.now() + link.expires_in * 1000;
      while (Date.now() < until) {
        await new Promise((r) => setTimeout(r, Math.max(2, link.interval) * 1000));
        const poll = await call<{ state: "pending" | "done"; name?: string }>("cloud_link_poll", { server, device_code: link.device_code });
        if (poll.state === "done") {
          toast(t("cl.signedIn", poll.name ?? ""));
          break;
        }
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      running.current = false;
      setLinking(null);
      setLinkUrl(null);
      await reload();
    }
  }, [server, toast, reload]);

  const signOut = useCallback(async () => {
    await call("cloud_sign_out").catch((e) => toast(errorText(e), "error"));
    await reload();
  }, [toast, reload]);

  const account = stored && clean(stored.server) === server ? stored : null;
  return <Context.Provider value={{ account, linking, linkUrl, signIn, signOut, reload }}>{props.children}</Context.Provider>;
}

/** The Discord picture, or the first letter when there is none or it does not load. */
export function Avatar(props: { account: CloudAccount; size?: number }) {
  const [broken, setBroken] = useState(false);
  const size = props.size ?? 44;
  const style = { width: size, height: size, fontSize: Math.round(size * 0.42) };
  return props.account.avatar && !broken ? (
    <img className="cloud-avatar" style={style} src={props.account.avatar} alt="" draggable={false} onError={() => setBroken(true)} />
  ) : (
    <span className="cloud-avatar cloud-avatar-letter" style={style}>
      {props.account.name.slice(0, 1).toUpperCase() || "?"}
    </span>
  );
}
