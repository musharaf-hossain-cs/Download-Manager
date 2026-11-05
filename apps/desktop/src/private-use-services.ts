import type { TauriInvoke } from "./tauri-transport.js";

export interface DesktopPreferences {
  readonly last_download_directory: string | null;
}

export interface DesktopDiagnostics {
  readonly product_version: string;
  readonly backend_health: string;
  readonly state_path: string;
  readonly log_path: string;
}

/** Native current-user services kept outside the downloadable engine protocol. */
export class PrivateUseDesktopServices {
  public constructor(private readonly invoke: TauriInvoke) {}

  public loadPreferences(): Promise<DesktopPreferences> {
    return this.invoke(
      "load_desktop_preferences",
      {},
    ) as Promise<DesktopPreferences>;
  }

  public async saveLastDownloadDirectory(directory: string): Promise<void> {
    await this.invoke("save_last_download_directory", { directory });
  }

  public diagnostics(): Promise<DesktopDiagnostics> {
    return this.invoke(
      "desktop_diagnostics",
      {},
    ) as Promise<DesktopDiagnostics>;
  }

  public async openFile(path: string): Promise<void> {
    await this.invoke("open_local_file", { path });
  }

  public async revealFile(path: string): Promise<void> {
    await this.invoke("reveal_local_file", { path });
  }
}
