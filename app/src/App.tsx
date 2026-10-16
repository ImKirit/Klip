import { useEffect, useState } from "react";
import { call, errorText, isTauri } from "./lib/api";
import { cleanLang, setLang, t } from "./lib/i18n";
import { applyTheme, DEFAULT_ACCENT, normalizeSettings } from "./lib/settings";
import type { AppSettings, Config, Env } from "./lib/types";
import { AppProvider, useApp } from "./state";
import { AccountProvider, useAccount } from "./account";
import { Button, Modal, ToastHost } from "./ui";
import { Library } from "./views/Library";
import { Editor } from "./views/Editor";
import { Settings } from "./views/Settings";
import { Mark } from "./views/Shell";
import { runSelfTest } from "./selftest";

interface Boot {
  env: Env;
  config: Config;
  configError: string | null;
  settings: AppSettings;
}

/** Asked once after the first start: sign in with Discord, or go on without. */
function Welcome() {
  const { settings, saveSettings, env } = useApp();
  const { signIn } = useAccount();
  if (settings.welcomed || env.selftest) return null;
  const answer = (login: boolean) => {
    void saveSettings({ ...settings, welcomed: true });
    if (login) void signIn();
  };
  return (
    <Modal title={t("wel.title")} onClose={() => answer(false)} locked>
      <div className="welcome">
        <Mark size={64} />
        <p>{t("wel.text")}</p>
        <div className="welcome-actions">
          <Button kind="primary" onClick={() => answer(true)}>
            {t("cl.signIn")}
          </Button>
          <Button onClick={() => answer(false)}>{t("wel.skip")}</Button>
        </div>
        <p className="muted small">{t("wel.later")}</p>
      </div>
    </Modal>
  );
}

function Views() {
  const { route, env, clips, scanning, lang } = useApp();
  // The self test of the real app: load every test clip once, write a report, quit.
  useEffect(() => {
    if (env.selftest && !scanning) void runSelfTest(clips);
  }, [env.selftest, scanning, clips]);
  // The language is part of every key: a switch draws the whole view again, memoised
  // parts included.
  const view =
    route.view === "editor" ? <Editor key={`${lang}:${route.path}`} path={route.path} /> : route.view === "settings" ? <Settings key={lang} page={route.page} /> : <Library key={lang} />;
  return (
    <>
      {view}
      <Welcome />
    </>
  );
}

export function App() {
  const [boot, setBoot] = useState<Boot | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    applyTheme("dark", DEFAULT_ACCENT);
    (async () => {
      const env = await call<Env>("env");
      const loaded = await call<{ config: Config; exists: boolean; error: string | null }>("config_load");
      const settings = normalizeSettings(await call("settings_load"));
      setLang(cleanLang(loaded.config.language));
      applyTheme(settings.theme, loaded.config.accent);
      setBoot({ env, config: loaded.config, configError: loaded.error, settings });
    })().catch((error) => setFailure(errorText(error)));
  }, []);

  if (failure) {
    return (
      <div className="boot">
        <Mark size={72} />
        <p className="boot-error">{t("boot.failed", failure)}</p>
        {isTauri ? null : <p className="muted">{t("boot.needsBackend")}</p>}
      </div>
    );
  }
  if (!boot) return <div className="boot" />;
  return (
    <ToastHost>
      <AppProvider {...boot}>
        <AccountProvider>
          <Views />
        </AccountProvider>
      </AppProvider>
    </ToastHost>
  );
}
