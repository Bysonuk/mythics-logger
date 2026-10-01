// Everything the window asks of the app goes through a Bridge, so the views
// can be tested against a fake one without Tauri.

import type { History, SettingsPatch, Snapshot, UploadStatus, Visibility } from "./types";

export interface Bridge {
  getState(): Promise<Snapshot>;
  onChanged(cb: () => void): () => void;
  logIn(): Promise<void>;
  cancelLogIn(): Promise<void>;
  logOut(): Promise<void>;
  saveSettings(patch: SettingsPatch): Promise<Snapshot>;
  chooseLogsFolder(): Promise<string | null>;
  findLogsFolder(): Promise<string>;
  backlogScan(): Promise<void>;
  backlogChooseFiles(): Promise<number>;
  backlogCancel(): Promise<void>;
  backlogUpload(paths: string[], visibility: Visibility): Promise<number>;
  backlogPause(paused: boolean): Promise<void>;
  /** Archives one finished log into Logs\MythicsLogsArchive; the archive's name. */
  archiveLog(path: string): Promise<string>;
  /** Opens the archive folder in File Explorer. */
  openArchiveFolder(): Promise<void>;
  /** "Skip the rest of this log", or undoing it. */
  backlogSkip(path: string, skip: boolean): Promise<void>;
  history(): Promise<History>;
  /** The newest uploads' status and pages, in one request. */
  recentUploads(): Promise<UploadStatus[]>;
  setUploadVisibility(id: string, visibility: Visibility): Promise<void>;
  deleteUpload(id: string): Promise<void>;
  openSite(path?: string): Promise<void>;
  /** A log page on the site, as the server gave it: the log, a raid boss,
   *  a pull or a key. The app opens only those, on the site in Settings. */
  openLog(path: string): Promise<void>;
}

/** The real bridge, over Tauri's IPC. Loaded lazily so tests never import it. */
export async function tauriBridge(): Promise<Bridge> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    getState: () => invoke("get_state"),
    onChanged(cb) {
      let off: (() => void) | null = null;
      let gone = false;
      void listen("changed", () => cb()).then((u) => {
        if (gone) u();
        else off = u;
      });
      return () => {
        gone = true;
        off?.();
      };
    },
    logIn: () => invoke("log_in"),
    cancelLogIn: () => invoke("cancel_log_in"),
    logOut: () => invoke("log_out"),
    saveSettings: (patch) => invoke("save_settings", { patch }),
    chooseLogsFolder: () => invoke("choose_logs_folder"),
    findLogsFolder: () => invoke("find_logs_folder"),
    backlogScan: () => invoke("backlog_scan"),
    backlogChooseFiles: () => invoke("backlog_choose_files"),
    backlogCancel: () => invoke("backlog_cancel"),
    backlogUpload: (paths, visibility) => invoke("backlog_upload", { paths, visibility }),
    backlogPause: (paused) => invoke("backlog_pause", { paused }),
    archiveLog: (path) => invoke("archive_log", { path }),
    openArchiveFolder: () => invoke("open_archive_folder"),
    backlogSkip: (path, skip) => invoke("backlog_skip", { path, skip }),
    history: () => invoke("history"),
    recentUploads: () => invoke("recent_uploads"),
    setUploadVisibility: (id, visibility) => invoke("set_upload_visibility", { id, visibility }),
    deleteUpload: (id) => invoke("delete_upload", { id }),
    openSite: (path) => invoke("open_site", { path: path ?? null }),
    openLog: (path) => invoke("open_log", { path }),
  };
}
