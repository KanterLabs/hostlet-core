import { useCallback, useEffect, useRef, useState } from "react";
import type { useRouter } from "next/navigation";
import { useConfirm } from "@/components/ui";
import { api } from "@/lib/api";
import { isActiveDeploy, waitForAgentJob } from "./appDetailHelpers";
import { emptySettings, settingsEqual, settingsFromApp } from "./appDetail.types";
import type { App, BusyAction, RuntimeHealth, SettingsForm } from "./appDetail.types";

type Router = ReturnType<typeof useRouter>;

/**
 * Build-critical settings only reach the container on the next deploy. Returns
 * true when the just-saved form differs from the app's last-known build inputs
 * for any of those fields. Display-only toggles (public exposure, auto deploy)
 * are intentionally excluded — they apply immediately and never need a redeploy.
 */
function buildCriticalSettingsChanged(app: App | null, settings: SettingsForm) {
  if (!app) return false;
  return (
    settings.domain !== (app.domain || "") ||
    settings.runtime_kind !== (app.runtimeKind || "single") ||
    (settings.hostlet_config_path || "hostlet.yml") !== (app.hostletConfigPath || "hostlet.yml") ||
    settings.packaging_strategy !== (app.packagingStrategy || "auto") ||
    (settings.root_directory || ".") !== (app.rootDirectory || ".") ||
    (settings.build_command.trim() || "") !== (app.buildCommand || "") ||
    (settings.start_command.trim() || "") !== (app.startCommand || "") ||
    Number(settings.container_port) !== (app.containerPort ?? 3000) ||
    (settings.memory_limit_mb ? Number(settings.memory_limit_mb) : null) !== (app.memoryLimitMb ?? null) ||
    (settings.cpu_limit ? Number(settings.cpu_limit) : null) !== (app.cpuLimit ?? null)
  );
}

function mergeDirtySettings(current: SettingsForm, previousServer: SettingsForm, loaded: App) {
  const next = settingsFromApp(loaded);
  (Object.keys(emptySettings) as Array<keyof SettingsForm>).forEach((key) => {
    if (current[key] !== previousServer[key]) Object.assign(next, { [key]: current[key] });
  });
  return next;
}

type UseAppActionsArgs = {
  id: string;
  app: App | null;
  settings: SettingsForm;
  router: Router;
  setApp: (app: App) => void;
  setSettings: (settings: SettingsForm) => void;
  setHealth: (health: RuntimeHealth) => void;
  setEnvKeys: (keys: Array<{ key: string }>) => void;
  setEnvValues: (updater: (current: Record<string, string>) => Record<string, string>) => void;
  setNewEnv: (next: { key: string; value: string }) => void;
  refreshScreenshot: () => Promise<void>;
  settingsDirty: boolean;
  confirmNavigation: () => boolean;
};

/**
 * Owns the busy/message state and every async operation on the app detail page.
 * Each handler keeps its original behavior byte-for-byte (same routes, payloads,
 * messages, and busy gating); the shared try/catch/finally bookkeeping is the
 * only thing factored out.
 */
export function useAppActions({
  id,
  app,
  settings,
  router,
  setApp,
  setSettings,
  setHealth,
  setEnvKeys,
  setEnvValues,
  setNewEnv,
  refreshScreenshot,
  settingsDirty,
  confirmNavigation,
}: UseAppActionsArgs) {
  const confirmAction = useConfirm();
  const [message, setMessage] = useState("");
  const [healthMessage, setHealthMessage] = useState("Waiting for runtime health.");
  const [busyAction, setBusyAction] = useState<BusyAction>("");
  // Specific screenshot job error (the agent job's `failure` string) so the
  // detail page can surface it with a Retry affordance instead of the generic
  // "Screenshot capture failed." copy.
  const [screenshotError, setScreenshotError] = useState("");
  // Local "build-critical settings saved but not yet redeployed" signal. It is
  // set the moment such a save succeeds and cleared when a deploy is kicked off,
  // driving the "Settings changed — redeploy to apply" drift banner.
  const [settingsChangedSinceDeploy, setSettingsChangedSinceDeploy] = useState(false);

  // Tracks whether the component that owns this hook is still mounted; used to
  // guard state updates and navigation after async operations complete.
  const mountedRef = useRef(true);
  const appRef = useRef(app);
  const settingsRef = useRef(settings);
  appRef.current = app;
  settingsRef.current = settings;
  useEffect(() => () => { mountedRef.current = false; }, []);

  const refreshApp = useCallback(async (options: { syncSettings?: boolean; expectedSettings?: SettingsForm } = {}) => {
    try {
      const loaded = await api<App>(`/api/apps/${id}`);
      const previousServerDraft = appRef.current ? settingsFromApp(appRef.current) : emptySettings;
      const currentDraft = settingsRef.current;
      const draftDirty = !settingsEqual(currentDraft, previousServerDraft);
      const expectedStillCurrent = !options.expectedSettings || settingsEqual(currentDraft, options.expectedSettings);
      setApp(loaded);
      appRef.current = loaded;
      if (loaded.health) setHealth(loaded.health);
      // Auxiliary refreshes update the server snapshot but only refill a clean
      // form. A dirty draft is merged field-by-field, so server-side changes
      // (for example publishing or pausing) update clean controls without
      // replacing values the user has edited. Explicit save/initial-load syncs
      // are conditional on the submitted draft still being current, so a
      // keystroke during a request can never be lost.
      if (options.syncSettings && expectedStillCurrent && (!draftDirty || !!options.expectedSettings)) {
        setSettings(settingsFromApp(loaded));
      } else if (!draftDirty) {
        setSettings(settingsFromApp(loaded));
      } else {
        setSettings(mergeDirtySettings(currentDraft, previousServerDraft, loaded));
      }
      return loaded;
    } catch {
      setMessage("Could not load app. Sign in and check that it still exists.");
      return null;
    }
  }, [id, setApp, setHealth, setSettings]);

  const deploy = useCallback(async () => {
    if (busyAction || isActiveDeploy(app?.latestDeployment?.status)) return;
    setBusyAction("deploy");
    setMessage(settingsDirty ? "Starting deployment. Unsaved settings remain in this form and are not included in this action." : "Starting deployment...");
    try {
      const res = await api<{ deploymentId: string }>(`/api/apps/${id}/deploy`, { method: "POST", body: "{}" });
      setSettingsChangedSinceDeploy(false);
      if (!confirmNavigation()) {
        setMessage("Deployment started. Unsaved settings remain in this form and were not included.");
        setBusyAction("");
        return;
      }
      router.push(`/deployments/${res.deploymentId}`);
    } catch (error) {
      setMessage(`Deploy failed to start. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }, [app?.latestDeployment?.status, busyAction, confirmNavigation, id, router, settingsDirty]);

  const rollback = useCallback(async () => {
    if (busyAction) return;
    setBusyAction("rollback");
    setMessage(settingsDirty ? "Starting rollback. Unsaved settings remain in this form and are not included in this action." : "Starting rollback...");
    try {
      const res = await api<{ rollbackDeploymentId: string }>(`/api/apps/${id}/rollback`, { method: "POST", body: "{}" });
      if (!confirmNavigation()) {
        setMessage("Rollback started. Unsaved settings remain in this form and were not included.");
        setBusyAction("");
        return;
      }
      router.push(`/deployments/${res.rollbackDeploymentId}`);
    } catch (error) {
      setMessage(`Rollback could not start. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }, [busyAction, confirmNavigation, id, router, settingsDirty]);

  const deleteApp = useCallback(async () => {
    if (!(await confirmAction({
      title: "Delete app?",
      description: "Delete this app, its Hostlet-managed route, containers, images, and deployment history?",
      confirmLabel: "Delete",
      destructive: true,
    }))) return;
    if (!confirmNavigation()) return;
    if (busyAction) return;
    setBusyAction("delete");
    setMessage("Deleting app and requesting server cleanup...");
    try {
      const result = await api<{ jobId?: string } | undefined>(`/api/apps/${id}`, { method: "DELETE" });
      if (result?.jobId) {
        setMessage("Server cleanup is running...");
        await waitForAgentJob(result.jobId, setMessage, () => mountedRef.current);
      }
      if (!mountedRef.current) return;
      router.push("/apps");
    } catch (error) {
      setMessage(`Delete failed. ${error instanceof Error ? error.message : ""}`);
      setBusyAction("");
    }
  }, [busyAction, confirmAction, confirmNavigation, id, router]);

  const toggleExposure = useCallback(async () => {
    if (!app || busyAction) return;
    const next = !app.publicExposure;
    setBusyAction("exposure");
    setMessage(next ? "Publishing app URL..." : "Making app private...");
    try {
      await api(`/api/apps/${id}`, { method: "PATCH", body: JSON.stringify({ public_exposure: next }) });
      await refreshApp();
      setMessage(`${next ? "App URL published. DNS may take a moment to propagate." : "App URL is private."}${settingsDirty ? " Unsaved settings remain in this form and were not included." : ""}`);
    } catch (error) {
      setMessage(`${next ? "Publish" : "Unpublish"} failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }, [app, busyAction, id, refreshApp, settingsDirty]);

  const saveSettings = useCallback(async () => {
    if (busyAction) return;
    setBusyAction("settings");
    setMessage("Saving app settings...");
    try {
      const payload: Record<string, unknown> = {
        health_path: settings.health_path,
        root_directory: settings.root_directory || ".",
        packaging_strategy: settings.packaging_strategy,
        install_command: null,
        build_command: settings.build_command.trim() || null,
        start_command: settings.start_command.trim() || null,
        container_port: Number(settings.container_port),
      };
      payload.domain = settings.domain;
      payload.runtime_kind = settings.runtime_kind;
      payload.hostlet_config_path = settings.hostlet_config_path || "hostlet.yml";
      payload.memory_limit_mb = settings.memory_limit_mb ? Number(settings.memory_limit_mb) : null;
      payload.cpu_limit = settings.cpu_limit ? Number(settings.cpu_limit) : null;
      payload.public_exposure = settings.public_exposure;
      payload.auto_deploy = settings.auto_deploy;
      await api(`/api/apps/${id}`, {
        method: "PATCH",
        body: JSON.stringify(payload),
      });
      if (buildCriticalSettingsChanged(app, settings)) setSettingsChangedSinceDeploy(true);
      await refreshApp({ syncSettings: true, expectedSettings: settings });
      setMessage("Settings saved. Redeploy for runtime changes to reach the container.");
    } catch (error) {
      setMessage(`Save failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }, [app, busyAction, settings, id, refreshApp]);

  const saveEnvVar = useCallback(
    async (key: string, value: string) => {
      if (busyAction || !key.trim()) return;
      setBusyAction("env");
      setMessage("Saving environment variable...");
      try {
        await api(`/api/apps/${id}/env/${encodeURIComponent(key.trim().toUpperCase())}`, {
          method: "PUT",
          body: JSON.stringify({ value }),
        });
        setEnvKeys(await api<Array<{ key: string }>>(`/api/apps/${id}/env`));
        setEnvValues((current) => ({ ...current, [key]: "" }));
        setNewEnv({ key: "", value: "" });
        setMessage("Environment variable saved. Redeploy for the change to reach the container.");
      } catch (error) {
        setMessage(`Env save failed. ${error instanceof Error ? error.message : ""}`);
      } finally {
        setBusyAction("");
      }
    },
    [busyAction, id, setEnvKeys, setEnvValues, setNewEnv],
  );

  const checkHealthNow = useCallback(async () => {
    if (busyAction) return;
    setBusyAction("health");
    setHealthMessage("Requesting a fresh health check...");
    try {
      await api(`/api/apps/${id}/health/check-now`, { method: "POST", body: "{}" });
      setHealthMessage("Health check requested. Waiting for the agent result...");
    } catch (error) {
      setHealthMessage(`Health check could not start. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }, [busyAction, id]);

  const checkBrowserNow = useCallback(async () => {
    if (busyAction) return;
    setBusyAction("browser");
    setHealthMessage("Opening the public app in a real browser...");
    try {
      const result = await api<{ jobId: string }>(`/api/apps/${id}/browser-check`, {
        method: "POST",
        body: "{}",
      });
      await waitForAgentJob(result.jobId, setHealthMessage, () => mountedRef.current);
      if (!mountedRef.current) return;
      await refreshApp();
      setHealthMessage(settingsDirty
        ? "Browser check completed. Unsaved settings remain in this form and were not included."
        : "Browser check completed.");
    } catch (error) {
      setHealthMessage(`Browser check failed. ${error instanceof Error ? error.message : ""}`);
    } finally {
      if (mountedRef.current) setBusyAction("");
    }
  }, [busyAction, id, refreshApp, settingsDirty]);

  const restartContainer = useCallback(async () => {
    if (busyAction || !(await confirmAction({
      title: "Restart app?",
      description: "Restart the current app container?",
      confirmLabel: "Restart",
    }))) return;
    setBusyAction("restart");
    setHealthMessage("Requesting container restart...");
    try {
      await api(`/api/apps/${id}/restart`, { method: "POST", body: "{}" });
      setHealthMessage(settingsDirty
        ? "Container restart requested. Unsaved settings remain in this form and were not included. Waiting for the agent health result..."
        : "Container restart requested. Waiting for the agent health result...");
    } catch (error) {
      setHealthMessage(`Restart could not start. ${error instanceof Error ? error.message : ""}`);
    } finally {
      setBusyAction("");
    }
  }, [busyAction, confirmAction, id, settingsDirty]);

  const togglePause = useCallback(async () => {
    if (!app || busyAction) return;
    const paused = Boolean(app.suspendedAt);
    if (!(await confirmAction({
      title: paused ? "Resume app?" : "Pause app?",
      description: paused
        ? "Start the preserved app runtime again? Other suspension reasons can still keep it paused."
        : "Stop every service for this app while preserving its containers and volumes?",
      confirmLabel: paused ? "Resume" : "Pause",
    }))) return;
    setBusyAction(paused ? "resume" : "pause");
    setHealthMessage(paused ? "Requesting app resume..." : "Requesting app pause...");
    try {
      await api(`/api/apps/${id}/${paused ? "resume" : "pause"}`, {
        method: "POST",
        body: "{}",
      });
      await refreshApp();
      setHealthMessage(`${paused ? "App resume requested." : "App paused."}${settingsDirty ? " Unsaved settings remain in this form and were not included." : ""}`);
    } catch (error) {
      setHealthMessage(
        `${paused ? "Resume" : "Pause"} could not start. ${error instanceof Error ? error.message : ""}`,
      );
    } finally {
      if (mountedRef.current) setBusyAction("");
    }
  }, [app, busyAction, confirmAction, id, refreshApp, settingsDirty]);

  const captureScreenshot = useCallback(async () => {
    if (busyAction) return;
    setBusyAction("screenshot");
    setScreenshotError("");
    setMessage("Requesting screenshot capture...");
    try {
      const result = await api<{ jobId: string }>(`/api/apps/${id}/screenshots`, { method: "POST", body: "{}" });
      setMessage("Screenshot capture is running...");
      await waitForAgentJob(result.jobId, setMessage, () => mountedRef.current);
      if (!mountedRef.current) return;
      await refreshScreenshot();
      if (!mountedRef.current) return;
      setMessage("Screenshot captured.");
    } catch (error) {
      // Surface the specific job error (the agent job's `failure` string bubbles
      // up through waitForAgentJob) rather than a hardcoded capture-failed line.
      const detail = error instanceof Error ? error.message : "Screenshot capture failed.";
      setScreenshotError(detail);
      setMessage(detail);
    } finally {
      setBusyAction("");
    }
  }, [busyAction, id, refreshScreenshot]);

  const deleteEnvVar = useCallback(
    async (key: string) => {
      if (busyAction || !(await confirmAction({
        title: "Delete environment variable?",
        description: `Delete ${key}?`,
        confirmLabel: "Delete",
        destructive: true,
      }))) return;
      setBusyAction("env");
      setMessage("Deleting environment variable...");
      try {
        await api(`/api/apps/${id}/env/${encodeURIComponent(key)}`, { method: "DELETE" });
        setEnvKeys(await api<Array<{ key: string }>>(`/api/apps/${id}/env`));
        setMessage("Environment variable deleted. Redeploy for the change to reach the container.");
      } catch (error) {
        setMessage(`Env delete failed. ${error instanceof Error ? error.message : ""}`);
      } finally {
        setBusyAction("");
      }
    },
    [busyAction, confirmAction, id, setEnvKeys],
  );

  return {
    message,
    setMessage,
    healthMessage,
    setHealthMessage,
    busyAction,
    screenshotError,
    settingsChangedSinceDeploy,
    refreshApp,
    deploy,
    rollback,
    deleteApp,
    toggleExposure,
    saveSettings,
    saveEnvVar,
    checkHealthNow,
    checkBrowserNow,
    restartContainer,
    togglePause,
    captureScreenshot,
    deleteEnvVar,
  };
}
