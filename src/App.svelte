<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { Rewind, FastForward, Play, Pause } from "phosphor-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { listen } from "@tauri-apps/api/event";

  const nav = ["Search", "Library", "Downloads", "Settings"];
  const PAGE_SIZE = 25;
  const SETTINGS_STORAGE_KEY = "soulglass-settings";

  type SearchResult = {
    username: string;
    folder: string;
    name: string;
    size: number;
    attributes: string;
    duration_seconds?: number;
  };

  type AlbumGroup = {
    id: string;
    username: string;
    folder: string;
    albumName: string;
    files: SearchResult[];
    totalSize: number;
  };

  type DownloadStatus =
    | "queued"
    | "downloading"
    | "paused"
    | "done"
    | "failed"
    | "deleted";

  type DownloadItem = {
    id: string;
    packageId: string;
    packageName: string;
    username: string;
    folder: string;
    name: string;
    size: number;
    attributes: string;
    duration_seconds?: number;
    progress: number;
    status: DownloadStatus;
    priority: number;
    error?: string;
    speed_bytes_per_sec?: number;
  };

  type DownloadGroup = {
    packageId: string;
    packageName: string;
    items: DownloadItem[];
    totalSize: number;
    downloadedSize: number;
    progress: number;
    status: DownloadStatus;
  };

  type ContextMenuItem = {
    label: string;
    action: () => void | Promise<void>;
    disabled?: boolean;
  };

  type AppSettings = {
    soulseekUsername: string;
    soulseekPassword: string;
    downloadFolder: string;
    searchTimeoutSeconds: number;
  };

  type DownloadEvent = {
    id: string;
    username: string;
    folder: string;
    name: string;
    size: number;
    status: DownloadStatus;
    progress: number;
    speed_bytes_per_sec?: number;
    error?: string;
  };

  type LibraryTrack = {
    id: string;
    name: string;
    path: string;
    folder: string;
    album_name: string;
    size: number;
    extension: string;
    cover_data_url?: string;
  };

  type LibraryAlbum = {
    id: string;
    albumName: string;
    folder: string;
    tracks: LibraryTrack[];
    totalSize: number;
    cover_data_url?: string;
    isSingle: boolean;
  };

  let editingAlbum = $state<LibraryAlbum | null>(null);
  let editAlbumName = $state("");
  let editTrackNames = $state<Record<string, string>>({});

  let expandedLibraryAlbums = $state(new Set<string>());
  let albumCoverCache = $state(new Map<string, string | null>());
  let loadingAlbumCovers = new Set<string>();

  let libraryTracks = $state<LibraryTrack[]>([]);
  let libraryError = $state("");
  let libraryLoading = $state(false);

  let unlistenSearchResults: null | (() => void) = null;
  let unlistenDownloads: null | (() => void) = null;

  let active = $state("Search");
  let activeSearchQuery = $state("");
  let searchQuery = $state("");
  let searching = $state(false);
  let searchError = $state("");
  let results = $state<SearchResult[]>([]);
  let visibleCount = $state(PAGE_SIZE);
  let expandedFolders = $state(new Set<string>());

  let settings = $state<AppSettings>({
    soulseekUsername: "",
    soulseekPassword: "",
    downloadFolder: "",
    searchTimeoutSeconds: 10,
  });

  let downloadQueue = $state<DownloadItem[]>([]);

  let contextMenuOpen = $state(false);
  let contextMenuX = $state(0);
  let contextMenuY = $state(0);
  let contextMenuItems = $state<ContextMenuItem[]>([]);

  let audioRef = $state<HTMLAudioElement | null>(null);
  let currentTrack = $state<LibraryTrack | null>(null);
  let isPlaying = $state(false);
  let playerTime = $state(0);
  let playerDuration = $state(0);
  let audioSrc = $state("");

  let playerProgressPercent = $derived(
    playerDuration > 0 ? (playerTime / playerDuration) * 100 : 0,
  );

  let groupedResults = $derived(groupResults(results));
  let visibleGroups = $derived(groupedResults.slice(0, visibleCount));
  let hasMoreResults = $derived(visibleCount < groupedResults.length);
  let downloadGroups = $derived(groupDownloads(downloadQueue));
  let libraryAlbums = $derived(groupLibraryTracks(libraryTracks));

  onMount(() => {
    loadSettings();

    invoke("set_window_material", { material: "acrylic" }).catch(console.error);

    window.addEventListener("click", closeContextMenu);
    window.addEventListener("blur", closeContextMenu);
    window.addEventListener("keydown", onGlobalKeyDown);

    listen<{
      query: string;
      results: SearchResult[];
      done: boolean;
      error?: string;
    }>("soulseek-search-results", (event) => {
      if (event.payload.query !== activeSearchQuery) return;

      if (event.payload.error) {
        searchError = event.payload.error;
        searching = false;
        return;
      }

      if (event.payload.results.length) {
        results = event.payload.results;
      }

      if (event.payload.done) {
        searching = false;
      }
    }).then((unlisten) => {
      unlistenSearchResults = unlisten;
    });

    listen<DownloadEvent>("download-event", (event) => {
      const update = event.payload;

      if (update.status === "deleted") {
        downloadQueue = downloadQueue.filter((item) => item.id !== update.id);
        return;
      }

      downloadQueue = downloadQueue.map((item) =>
        item.id === update.id
          ? {
              ...item,
              status: update.status,
              progress: update.progress,
              speed_bytes_per_sec: update.speed_bytes_per_sec ?? 0,
              error: update.error,
            }
          : item,
      );
    }).then((unlisten) => {
      unlistenDownloads = unlisten;
    });
  });

  onDestroy(() => {
    window.removeEventListener("click", closeContextMenu);
    window.removeEventListener("blur", closeContextMenu);
    window.removeEventListener("keydown", onGlobalKeyDown);

    if (unlistenSearchResults) {
      unlistenSearchResults();
      unlistenSearchResults = null;
    }

    if (unlistenDownloads) {
      unlistenDownloads();
      unlistenDownloads = null;
    }
  });

  function openEditAlbum(album: LibraryAlbum) {
    editingAlbum = album;
    editAlbumName = album.albumName;

    editTrackNames = Object.fromEntries(
      album.tracks.map((track) => [
        track.id,
        track.name.replace(/\.[^/.]+$/, ""),
      ]),
    );
  }

  function closeEditAlbum() {
    editingAlbum = null;
    editAlbumName = "";
    editTrackNames = {};
  }

  async function saveEditedAlbum() {
    if (!editingAlbum) return;

    try {
      await invoke("edit_library_album", {
        req: {
          folder: editingAlbum.folder,
          album_name: editAlbumName,
          tracks: editingAlbum.tracks.map((track) => ({
            old_path: track.path,
            new_name: editTrackNames[track.id] ?? track.name,
          })),
        },
      });

      closeEditAlbum();
      await scanLibrary();
    } catch (err) {
      libraryError = String(err);
    }
  }

  function saveSettings() {
    localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(settings));
  }

  function loadSettings() {
    const raw = localStorage.getItem(SETTINGS_STORAGE_KEY);
    if (!raw) return;

    try {
      const saved = JSON.parse(raw);

      settings = {
        soulseekUsername: saved.soulseekUsername ?? "",
        soulseekPassword: saved.soulseekPassword ?? "",
        downloadFolder: saved.downloadFolder ?? "",
        searchTimeoutSeconds: saved.searchTimeoutSeconds ?? 10,
      };
    } catch {
      localStorage.removeItem(SETTINGS_STORAGE_KEY);
    }
  }

  function clearLoginInfo() {
    settings = {
      ...settings,
      soulseekUsername: "",
      soulseekPassword: "",
    };

    saveSettings();
  }

  async function loadAlbumCover(album: LibraryAlbum) {
    if (albumCoverCache.has(album.folder)) return;
    if (loadingAlbumCovers.has(album.folder)) return;

    loadingAlbumCovers.add(album.folder);

    try {
      const cover = await invoke<string | null>("load_album_cover", {
        req: {
          folder: album.folder,
        },
      });

      const next = new Map(albumCoverCache);
      next.set(album.folder, cover);
      albumCoverCache = next;
    } catch {
      const next = new Map(albumCoverCache);
      next.set(album.folder, null);
      albumCoverCache = next;
    } finally {
      loadingAlbumCovers.delete(album.folder);
    }
  }

  function getAlbumCover(album: LibraryAlbum) {
    return album.cover_data_url ?? albumCoverCache.get(album.folder) ?? null;
  }

  async function chooseDownloadFolder() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Choose download folder",
    });

    if (typeof selected === "string") {
      settings = {
        ...settings,
        downloadFolder: selected,
      };

      saveSettings();
    }
  }

  async function runSearch() {
    const query = searchQuery.trim();

    if (!settings.soulseekUsername || !settings.soulseekPassword || !query) {
      searchError = "Set your Soulseek login in Settings first.";
      active = "Settings";
      return;
    }

    activeSearchQuery = query;
    searching = true;
    searchError = "";
    results = [];
    visibleCount = PAGE_SIZE;
    expandedFolders = new Set();

    try {
      await invoke<void>("soulseek_search", {
        req: {
          username: settings.soulseekUsername,
          password: settings.soulseekPassword,
          query,
        },
      });

      saveSettings();
    } catch (err) {
      searchError = String(err);
      searching = false;
    }
  }

  function makeDownloadId(file: SearchResult) {
    return `${file.username}::${file.folder}::${file.name}::${file.size}`;
  }

  function getSoulseekPath(file: SearchResult) {
    if (!file.folder || file.folder === "Unknown folder") return file.name;
    return `${file.folder}\\${file.name}`;
  }

  function sanitizeFolderName(name: string) {
    return (
      name
        .replace(/[<>:"/\\|?*\x00-\x1F]/g, "_")
        .replace(/\.+$/g, "")
        .trim() || "Unknown Album"
    );
  }

  function getDownloadPackageName(group: AlbumGroup) {
    return sanitizeFolderName(group.albumName);
  }

  function getSingleTrackPackageName(file: SearchResult) {
    return sanitizeFolderName(file.name.replace(/\.[^/.]+$/, ""));
  }

  async function queueTrack(file: SearchResult) {
    if (!settings.downloadFolder) {
      active = "Settings";
      searchError = "Choose a download folder in Settings first.";
      return;
    }

    const id = makeDownloadId(file);
    if (downloadQueue.some((item) => item.id === id)) return;

    const packageName = getSingleTrackPackageName(file);
    const packageId = id;

    const item: DownloadItem = {
      id,
      packageId,
      packageName,
      username: file.username,
      folder: file.folder,
      name: file.name,
      size: file.size,
      attributes: file.attributes,
      duration_seconds: file.duration_seconds,
      progress: 0,
      status: "queued",
      priority: downloadQueue.length,
      speed_bytes_per_sec: 0,
    };

    downloadQueue = [...downloadQueue, item];
    active = "Downloads";

    try {
      await invoke("queue_downloads", {
        req: {
          username: settings.soulseekUsername,
          password: settings.soulseekPassword,
          download_folder: settings.downloadFolder,
          package_id: packageId,
          package_name: packageName,
          max_concurrent: 1,
          items: [
            {
              id,
              package_id: packageId,
              package_name: packageName,
              username: file.username,
              folder: file.folder,
              name: file.name,
              path: getSoulseekPath(file),
              size: file.size,
            },
          ],
        },
      });
    } catch (err) {
      markDownloadFailed(id, String(err));
    }
  }

  async function queueFolder(group: AlbumGroup) {
    if (!settings.downloadFolder) {
      active = "Settings";
      searchError = "Choose a download folder in Settings first.";
      return;
    }

    const packageName = getDownloadPackageName(group);
    const packageId = `${group.username}::${group.folder}`;

    const fresh = group.files.filter((file) => {
      const id = makeDownloadId(file);
      return !downloadQueue.some((item) => item.id === id);
    });

    if (!fresh.length) return;

    const newItems: DownloadItem[] = fresh.map((file, offset) => ({
      id: makeDownloadId(file),
      packageId,
      packageName,
      username: file.username,
      folder: file.folder,
      name: file.name,
      size: file.size,
      attributes: file.attributes,
      duration_seconds: file.duration_seconds,
      progress: 0,
      status: "queued",
      priority: downloadQueue.length + offset,
      speed_bytes_per_sec: 0,
    }));

    downloadQueue = [...downloadQueue, ...newItems];
    active = "Downloads";

    try {
      await invoke("queue_downloads", {
        req: {
          username: settings.soulseekUsername,
          password: settings.soulseekPassword,
          download_folder: settings.downloadFolder,
          package_id: packageId,
          package_name: packageName,
          max_concurrent: 1,
          items: fresh.map((file) => ({
            id: makeDownloadId(file),
            package_id: packageId,
            package_name: packageName,
            username: file.username,
            folder: file.folder,
            name: file.name,
            path: getSoulseekPath(file),
            size: file.size,
          })),
        },
      });
    } catch (err) {
      const message = String(err);

      downloadQueue = downloadQueue.map((item) =>
        item.packageId === packageId
          ? {
              ...item,
              status: "failed",
              error: message,
            }
          : item,
      );
    }
  }

  function markDownloadFailed(id: string, error: string) {
    downloadQueue = downloadQueue.map((item) =>
      item.id === id
        ? {
            ...item,
            status: "failed",
            error,
          }
        : item,
    );
  }

  async function pauseDownload(id: string) {
    const item = downloadQueue.find((item) => item.id === id);
    if (!item) return;

    try {
      if (item.status === "paused") {
        await invoke("resume_download", { req: { id } });
      } else {
        await invoke("pause_download", { req: { id } });
      }
    } catch (err) {
      markDownloadFailed(id, String(err));
    }
  }

  async function deleteDownload(id: string) {
    downloadQueue = downloadQueue.filter((item) => item.id !== id);

    try {
      await invoke("delete_download", {
        req: {
          id,
        },
      });
    } catch {
      // Backend may already have removed it.
    }
  }

  async function deleteDownloadGroup(packageId: string) {
    const ids = downloadQueue
      .filter((item) => item.packageId === packageId)
      .map((item) => item.id);

    downloadQueue = downloadQueue.filter(
      (item) => item.packageId !== packageId,
    );

    await Promise.allSettled(
      ids.map((id) =>
        invoke("delete_download", {
          req: {
            id,
          },
        }),
      ),
    );
  }

  async function pauseDownloadGroup(packageId: string) {
    const groupItems = downloadQueue.filter(
      (item) => item.packageId === packageId,
    );
    const shouldResume = groupItems.every((item) => item.status === "paused");

    for (const item of groupItems) {
      try {
        await invoke(shouldResume ? "resume_download" : "pause_download", {
          req: { id: item.id },
        });
      } catch {
        // active downloads may not support pausing yet
      }
    }

    downloadQueue = downloadQueue.map((item) =>
      item.packageId === packageId
        ? {
            ...item,
            status: shouldResume ? "queued" : "paused",
          }
        : item,
    );
  }

  async function moveDownload(id: string, direction: -1 | 1) {
    const index = downloadQueue.findIndex((item) => item.id === id);
    if (index < 0) return;

    const target = index + direction;
    if (target < 0 || target >= downloadQueue.length) return;

    const next = [...downloadQueue];
    const temp = next[index];
    next[index] = next[target];
    next[target] = temp;

    downloadQueue = next.map((item, priority) => ({
      ...item,
      priority,
    }));

    await invoke("reorder_downloads", {
      req: {
        ids: downloadQueue.map((item) => item.id),
      },
    });
  }

  function getAlbumName(folder: string) {
    return folder.split(/[\\/]/).filter(Boolean).pop() ?? folder;
  }

  function groupResults(files: SearchResult[]): AlbumGroup[] {
    const map = new Map<string, AlbumGroup>();

    for (const file of files) {
      const id = `${file.username}::${file.folder}`;

      if (!map.has(id)) {
        map.set(id, {
          id,
          username: file.username,
          folder: file.folder,
          albumName: getAlbumName(file.folder),
          files: [],
          totalSize: 0,
        });
      }

      const group = map.get(id)!;
      group.files.push(file);
      group.totalSize += file.size;
    }

    return Array.from(map.values())
      .map((group) => ({
        ...group,
        files: group.files.sort((a, b) =>
          a.name.localeCompare(b.name, undefined, { numeric: true }),
        ),
      }))
      .sort((a, b) => b.files.length - a.files.length);
  }

  function groupDownloads(items: DownloadItem[]): DownloadGroup[] {
    const map = new Map<string, DownloadGroup>();

    for (const item of items) {
      if (!map.has(item.packageId)) {
        map.set(item.packageId, {
          packageId: item.packageId,
          packageName: item.packageName,
          items: [],
          totalSize: 0,
          downloadedSize: 0,
          progress: 0,
          status: "queued",
        });
      }

      const group = map.get(item.packageId)!;
      group.items.push(item);
      group.totalSize += item.size;
      group.downloadedSize += item.size * (item.progress / 100);
    }

    return Array.from(map.values())
      .map((group) => {
        group.progress = group.totalSize
          ? (group.downloadedSize / group.totalSize) * 100
          : 0;

        if (group.items.some((item) => item.status === "downloading")) {
          group.status = "downloading";
        } else if (group.items.some((item) => item.status === "failed")) {
          group.status = "failed";
        } else if (group.items.every((item) => item.status === "done")) {
          group.status = "done";
        } else if (group.items.every((item) => item.status === "paused")) {
          group.status = "paused";
        } else {
          group.status = "queued";
        }

        group.items.sort((a, b) => {
          const statusOrder: Record<DownloadStatus, number> = {
            downloading: 0,
            queued: 1,
            paused: 2,
            failed: 3,
            done: 4,
            deleted: 5,
          };

          return (
            statusOrder[a.status] - statusOrder[b.status] ||
            a.priority - b.priority ||
            a.name.localeCompare(b.name, undefined, { numeric: true })
          );
        });

        return group;
      })
      .sort((a, b) => {
        const statusOrder: Record<DownloadStatus, number> = {
          downloading: 0,
          queued: 1,
          paused: 2,
          failed: 3,
          done: 4,
          deleted: 5,
        };

        return (
          statusOrder[a.status] - statusOrder[b.status] ||
          Math.min(...a.items.map((item) => item.priority)) -
            Math.min(...b.items.map((item) => item.priority)) ||
          a.packageName.localeCompare(b.packageName, undefined, {
            numeric: true,
          })
        );
      });
  }

  function groupLibraryTracks(tracks: LibraryTrack[]): LibraryAlbum[] {
    const map = new Map<string, LibraryAlbum>();

    for (const track of tracks) {
      const id = track.folder || track.path;

      if (!map.has(id)) {
        map.set(id, {
          id,
          albumName: track.album_name || "Unknown Album",
          folder: track.folder,
          tracks: [],
          totalSize: 0,
          cover_data_url: track.cover_data_url,
          isSingle: false,
        });
      }

      const album = map.get(id)!;
      album.tracks.push(track);
      album.totalSize += track.size;

      if (!album.cover_data_url && track.cover_data_url) {
        album.cover_data_url = track.cover_data_url;
      }
    }

    return Array.from(map.values())
      .map((album) => {
        album.tracks.sort((a, b) =>
          a.name.localeCompare(b.name, undefined, { numeric: true }),
        );

        album.isSingle = album.tracks.length === 1;

        if (album.isSingle) {
          album.albumName = album.tracks[0].name.replace(/\.[^/.]+$/, "");
        }

        return album;
      })
      .sort((a, b) => {
        if (a.isSingle !== b.isSingle) return a.isSingle ? 1 : -1;
        return a.albumName.localeCompare(b.albumName, undefined, {
          numeric: true,
        });
      });
  }

  async function deleteLibraryAlbum(album: LibraryAlbum) {
    if (!settings.downloadFolder || album.folder === settings.downloadFolder) {
      libraryError = "Refusing to delete the root library folder.";
      return;
    }

    const confirmed = confirm(
      `Delete "${album.albumName}" from your library?\n\nThis will permanently delete:\n${album.folder}`,
    );

    if (!confirmed) return;

    try {
      await invoke("delete_library_album", {
        req: {
          folder: album.folder,
        },
      });

      libraryTracks = libraryTracks.filter(
        (track) => track.folder !== album.folder,
      );

      if (currentTrack && currentTrack.folder === album.folder) {
        if (audioRef) {
          audioRef.pause();
          audioRef.src = "";
        }

        currentTrack = null;
        audioSrc = "";
        isPlaying = false;
        playerTime = 0;
        playerDuration = 0;
      }
    } catch (err) {
      libraryError = String(err);
    }
  }

  function toggleLibraryAlbum(id: string) {
    const next = new Set(expandedLibraryAlbums);

    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }

    expandedLibraryAlbums = next;
  }

  async function scanLibrary() {
    if (!settings.downloadFolder) {
      active = "Settings";
      libraryError = "Choose a download folder in Settings first.";
      return;
    }

    libraryLoading = true;
    libraryError = "";

    try {
      libraryTracks = await invoke<LibraryTrack[]>("scan_library", {
        req: {
          download_folder: settings.downloadFolder,
        },
      });

      expandedLibraryAlbums = new Set();

      albumCoverCache = new Map();

      setTimeout(() => {
        for (const album of libraryAlbums.slice(0, 80)) {
          loadAlbumCover(album);
        }
      }, 0);
    } catch (err) {
      libraryError = String(err);
    } finally {
      libraryLoading = false;
    }
  }

  function toggleFolder(id: string) {
    const next = new Set(expandedFolders);

    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }

    expandedFolders = next;
  }

  function loadMoreResults() {
    if (!hasMoreResults || searching) return;
    visibleCount = Math.min(visibleCount + PAGE_SIZE, groupedResults.length);
  }

  function onResultsScroll(event: Event) {
    const el = event.currentTarget as HTMLElement;
    const nearBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 80;

    if (nearBottom) {
      loadMoreResults();
    }
  }

  function openContextMenu(event: MouseEvent, items: ContextMenuItem[]) {
    event.preventDefault();
    event.stopPropagation();

    contextMenuItems = items;
    contextMenuOpen = true;

    const menuWidth = 220;
    const menuHeight = Math.max(44, items.length * 38 + 12);

    contextMenuX = Math.min(event.clientX, window.innerWidth - menuWidth - 8);
    contextMenuY = Math.min(event.clientY, window.innerHeight - menuHeight - 8);
  }

  function closeContextMenu() {
    contextMenuOpen = false;
  }

  async function runContextAction(item: ContextMenuItem) {
    if (item.disabled) return;

    await item.action();
    closeContextMenu();
  }

  async function updateTaskbarProgress(groups: DownloadGroup[]) {
    const activeGroups = groups.filter(
      (group) =>
        group.status === "downloading" ||
        group.status === "queued" ||
        group.status === "paused" ||
        group.status === "failed",
    );

    if (!activeGroups.length) {
      await invoke("set_taskbar_progress", {
        req: {
          status: "none",
          progress: null,
        },
      }).catch(() => {});

      return;
    }

    const hasFailed = activeGroups.some((group) => group.status === "failed");
    const hasPaused = activeGroups.some((group) => group.status === "paused");
    const hasDownloading = activeGroups.some(
      (group) => group.status === "downloading",
    );

    const totalSize = activeGroups.reduce(
      (sum, group) => sum + group.totalSize,
      0,
    );

    const downloadedSize = activeGroups.reduce(
      (sum, group) => sum + group.downloadedSize,
      0,
    );

    const progress =
      totalSize > 0 ? Math.round((downloadedSize / totalSize) * 100) : 0;

    await invoke("set_taskbar_progress", {
      req: {
        status: hasFailed
          ? "error"
          : hasPaused && !hasDownloading
            ? "paused"
            : "normal",
        progress: Math.min(Math.max(progress, 0), 100),
      },
    }).catch(() => {});
  }

  $effect(() => {
    updateTaskbarProgress(downloadGroups);
  });

  function getCurrentTrackCover() {
    if (!currentTrack) return null;

    return (
      currentTrack.cover_data_url ??
      albumCoverCache.get(currentTrack.folder) ??
      null
    );
  }

  function formatBytes(bytes: number) {
    if (!bytes) return "0.0 MB";

    const mb = bytes / 1024 / 1024;
    return mb < 1024 ? `${mb.toFixed(1)} MB` : `${(mb / 1024).toFixed(2)} GB`;
  }

  function formatEta(seconds?: number) {
    if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return "ETA --";

    const rounded = Math.ceil(seconds);

    if (rounded < 60) {
      return `ETA ${rounded}s`;
    }

    const mins = Math.floor(rounded / 60);
    const secs = rounded % 60;

    if (mins < 60) {
      return `ETA ${mins}:${secs.toString().padStart(2, "0")}`;
    }

    const hours = Math.floor(mins / 60);
    const remMins = mins % 60;

    return `ETA ${hours}h ${remMins}m`;
  }

  function getItemRemainingBytes(item: DownloadItem) {
    return Math.max(item.size - item.size * (item.progress / 100), 0);
  }

  function getItemEta(item: DownloadItem) {
    const speed = item.speed_bytes_per_sec ?? 0;
    if (item.status !== "downloading" || speed <= 0) return undefined;

    return getItemRemainingBytes(item) / speed;
  }

  function getGroupRemainingBytes(group: DownloadGroup) {
    return Math.max(group.totalSize - group.downloadedSize, 0);
  }

  function getGroupEta(group: DownloadGroup) {
    const speed = getGroupSpeed(group);
    if (group.status !== "downloading" || speed <= 0) return undefined;

    return getGroupRemainingBytes(group) / speed;
  }

  function formatSpeed(bytesPerSecond?: number) {
    if (!bytesPerSecond || bytesPerSecond <= 0) return "0.0 MB/s";

    return `${(bytesPerSecond / 1024 / 1024).toFixed(1)} MB/s`;
  }

  function getGroupSpeed(group: DownloadGroup) {
    return group.items.reduce(
      (sum, item) => sum + (item.speed_bytes_per_sec ?? 0),
      0,
    );
  }

  function formatDuration(seconds?: number) {
    if (!seconds) return "";

    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;

    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }

  function getTrackTitle(track: LibraryTrack | null) {
    if (!track) return "Nothing playing";
    return track.name.replace(/\.[^/.]+$/, "");
  }

  function getTrackSubtitle(track: LibraryTrack | null) {
    if (!track) return "Select a track from your library";
    return track.album_name;
  }

  function onGlobalKeyDown(event: KeyboardEvent) {
    if (event.code !== "Space") return;

    const target = event.target as HTMLElement | null;
    const tagName = target?.tagName.toLowerCase();

    const isTyping =
      tagName === "input" ||
      tagName === "textarea" ||
      tagName === "select" ||
      target?.isContentEditable;

    if (isTyping) return;

    event.preventDefault();
    togglePlayer();
  }

  async function playLibraryTrack(track: LibraryTrack) {
    currentTrack = track;
    const matchingAlbum = libraryAlbums.find((album) =>
      album.tracks.some((albumTrack) => albumTrack.id === track.id),
    );

    if (matchingAlbum) {
      loadAlbumCover(matchingAlbum);
    }
    isPlaying = false;
    playerTime = 0;
    playerDuration = 0;
    audioSrc = "";

    await tick();

    if (!audioRef) return;

    try {
      let dataUrl = await invoke<string>("read_audio_data_url", {
        req: {
          path: track.path,
        },
      });

      if (track.extension.toLowerCase() === "m4a") {
        dataUrl = dataUrl.replace(/^data:audio\/x-m4a/i, "data:audio/mp4");
      }

      audioSrc = dataUrl;

      await tick();

      audioRef.currentTime = 0;
      audioRef.load();

      await audioRef.play();
      isPlaying = true;
    } catch (err) {
      console.error("failed to play track", err);
      isPlaying = false;
    }
  }

  async function togglePlayer() {
    if (!audioRef) return;

    if (!currentTrack) {
      const first = libraryTracks[0];
      if (first) await playLibraryTrack(first);
      return;
    }

    if (audioRef.paused) {
      await audioRef.play();
      isPlaying = true;
    } else {
      audioRef.pause();
      isPlaying = false;
    }
  }

  async function playAdjacentTrack(direction: -1 | 1) {
    if (!libraryTracks.length) return;

    if (!currentTrack) {
      await playLibraryTrack(libraryTracks[0]);
      return;
    }

    const index = libraryTracks.findIndex(
      (track) => track.id === currentTrack?.id,
    );
    const nextIndex =
      index < 0
        ? 0
        : Math.min(Math.max(index + direction, 0), libraryTracks.length - 1);

    await playLibraryTrack(libraryTracks[nextIndex]);
  }

  function onAudioTimeUpdate() {
    if (!audioRef) return;
    playerTime = audioRef.currentTime || 0;
  }

  function onAudioLoadedMetadata() {
    if (!audioRef) return;
    playerDuration = Number.isFinite(audioRef.duration) ? audioRef.duration : 0;
  }

  function onAudioEnded() {
    isPlaying = false;
    playAdjacentTrack(1);
  }

  function seekPlayer(event: MouseEvent) {
    if (!audioRef || !playerDuration) return;

    const el = event.currentTarget as HTMLElement;
    const rect = el.getBoundingClientRect();
    const ratio = Math.min(
      Math.max((event.clientX - rect.left) / rect.width, 0),
      1,
    );

    audioRef.currentTime = ratio * playerDuration;
    playerTime = audioRef.currentTime;
  }

  function formatPlayerTime(seconds: number) {
    if (!Number.isFinite(seconds) || seconds <= 0) return "0:00";

    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);

    return `${mins}:${secs.toString().padStart(2, "0")}`;
  }
</script>

<div class="app">
  <aside class="sidebar">
    <div class="brand">
      <div class="logo-dot"></div>
      <div>
        <strong>Soulglass</strong>
        <span>music client</span>
      </div>
    </div>

    <nav>
      {#each nav as item}
        <button
          class:active={active === item}
          onclick={() => {
            active = item;
            if (item === "Library") scanLibrary();
          }}
        >
          {item}
        </button>
      {/each}
    </nav>
  </aside>

  <section class="main">
    <header class="topbar" data-tauri-drag-region>
      <div data-tauri-drag-region>
        <h1>{active}</h1>
        <p data-tauri-drag-region>
          {#if active === "Search"}
            Search Soulseek and queue downloads.
          {:else if active === "Library"}
            Your imported, playable local music.
          {:else if active === "Settings"}
            Configure Soulseek, downloads, and app behavior.
          {:else}
            Active, queued, failed, and completed transfers.
          {/if}
        </p>
      </div>
    </header>

    <main class="content">
      {#if active === "Search"}
        <div class="search-box">
          <input
            placeholder="Search Soulseek..."
            bind:value={searchQuery}
            onkeydown={(e) => e.key === "Enter" && runSearch()}
          />

          <button onclick={runSearch}>
            {searching ? "Searching..." : "Search"}
          </button>
        </div>

        {#if searchError}
          <div class="empty-state">
            <h2>Search failed</h2>
            <p>{searchError}</p>
          </div>
        {:else if groupedResults.length}
          <div class="results" onscroll={onResultsScroll}>
            {#each visibleGroups as group}
              <div class="folder-group">
                <button
                  class="folder-row"
                  onclick={() => toggleFolder(group.id)}
                  oncontextmenu={(e) =>
                    openContextMenu(e, [
                      {
                        label: `Queue album (${group.files.length} songs)`,
                        action: () => queueFolder(group),
                      },
                      {
                        label: expandedFolders.has(group.id)
                          ? "Collapse folder"
                          : "Expand folder",
                        action: () => toggleFolder(group.id),
                      },
                      {
                        label: "Copy folder path",
                        action: () =>
                          navigator.clipboard.writeText(group.folder),
                      },
                      {
                        label: "Copy username",
                        action: () =>
                          navigator.clipboard.writeText(group.username),
                      },
                    ])}
                >
                  <div class="folder-main">
                    <span class="folder-arrow">
                      {expandedFolders.has(group.id) ? "▾" : "▸"}
                    </span>

                    <div>
                      <strong>{group.albumName}</strong>
                      <span>
                        {group.username} · {group.files.length} songs · {formatBytes(
                          group.totalSize,
                        )}
                      </span>
                    </div>
                  </div>

                  <small>{group.folder}</small>
                </button>

                {#if expandedFolders.has(group.id)}
                  <div class="track-list">
                    {#each group.files as file}
                      <div
                        class="track-row"
                        oncontextmenu={(e) =>
                          openContextMenu(e, [
                            {
                              label: "Queue track",
                              action: () => queueTrack(file),
                            },
                            {
                              label: "Copy filename",
                              action: () =>
                                navigator.clipboard.writeText(file.name),
                            },
                            {
                              label: "Copy username",
                              action: () =>
                                navigator.clipboard.writeText(file.username),
                            },
                          ])}
                      >
                        <div>
                          <strong>{file.name}</strong>
                          <span>{file.username}</span>
                        </div>

                        <div class="track-meta">
                          <span>{formatBytes(file.size)}</span>
                          {#if file.attributes}
                            <span>{file.attributes}</span>
                          {/if}
                          {#if file.duration_seconds}
                            <span>{formatDuration(file.duration_seconds)}</span>
                          {/if}
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}

            {#if hasMoreResults}
              <div class="load-more-hint">
                Scroll for more · showing {visibleGroups.length} of {groupedResults.length}
                folders
              </div>
            {:else}
              <div class="load-more-hint">
                End of results · {groupedResults.length} folders
              </div>
            {/if}
          </div>
        {:else}
          <div class="empty-state">
            <h2>Search music</h2>
            <p>Enter Soulseek credentials and search.</p>
          </div>
        {/if}
      {:else if active === "Library"}
        <div class="library-page">
          <div class="library-header">
            <div>
              <h2>Your library</h2>
              <p>
                {libraryAlbums.length} albums/singles · {libraryTracks.length} tracks
                found
              </p>
            </div>

            <button onclick={scanLibrary}>
              {libraryLoading ? "Scanning..." : "Refresh"}
            </button>
          </div>

          {#if libraryError}
            <div class="empty-state">
              <h2>Library error</h2>
              <p>{libraryError}</p>
            </div>
          {:else if libraryAlbums.length}
            <div class="library-grid">
              {#each libraryAlbums as album}
                <div
                  class:expanded={expandedLibraryAlbums.has(album.id)}
                  class="library-card library-album-card"
                  oncontextmenu={(e) =>
                    openContextMenu(e, [
                      {
                        label: album.isSingle ? "Delete song" : "Delete album",
                        action: () => deleteLibraryAlbum(album),
                      },
                      {
                        label: album.isSingle ? "Edit song" : "Edit album",
                        action: () => openEditAlbum(album),
                      },
                      {
                        label: "Copy folder path",
                        action: () =>
                          navigator.clipboard.writeText(album.folder),
                      },
                    ])}
                >
                  <button
                    class="library-card-button"
                    onclick={() =>
                      album.isSingle
                        ? playLibraryTrack(album.tracks[0])
                        : toggleLibraryAlbum(album.id)}
                    ondblclick={() => playLibraryTrack(album.tracks[0])}
                  >
                    <div class="library-cover">
                      {#if getAlbumCover(album)}
                        <img
                          src={getAlbumCover(album)!}
                          alt={album.albumName}
                        />
                      {:else}
                        <span>{album.albumName.slice(0, 1).toUpperCase()}</span>
                      {/if}
                    </div>

                    <div class="library-card-text">
                      <strong>{album.albumName}</strong>

                      {#if album.isSingle}
                        <span>{album.tracks[0].album_name}</span>
                        <small>
                          {album.tracks[0].extension.toUpperCase()} · {formatBytes(
                            album.totalSize,
                          )}
                        </small>
                      {:else}
                        <span>{album.tracks.length} tracks</span>
                        <small>{formatBytes(album.totalSize)}</small>
                      {/if}
                    </div>
                  </button>

                  {#if !album.isSingle && expandedLibraryAlbums.has(album.id)}
                    <div class="library-album-tracks">
                      {#each album.tracks as track}
                        <div
                          class="library-track-row"
                          ondblclick={() => playLibraryTrack(track)}
                        >
                          <button
                            class="track-play-button"
                            title="Play track"
                            onclick={(event) => {
                              event.stopPropagation();
                              playLibraryTrack(track);
                            }}
                          >
                            ▶
                          </button>

                          <div class="library-track-info">
                            <strong>{getTrackTitle(track)}</strong>
                            <span
                              >{track.extension.toUpperCase()} · {formatBytes(
                                track.size,
                              )}</span
                            >
                          </div>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {:else}
            <div class="empty-state">
              <h2>No music found</h2>
              <p>Downloaded tracks will appear here after they finish.</p>
            </div>
          {/if}
        </div>
      {:else if active === "Settings"}
        <div class="settings-page">
          <section class="settings-card">
            <div>
              <h2>Soulseek login</h2>
              <p>Used to connect and search. Saved locally on this device.</p>
            </div>

            <div class="settings-grid">
              <label>
                <span>Username</span>
                <input
                  placeholder="Soulseek username"
                  bind:value={settings.soulseekUsername}
                  onchange={saveSettings}
                />
              </label>

              <label>
                <span>Password</span>
                <input
                  placeholder="Soulseek password"
                  type="password"
                  bind:value={settings.soulseekPassword}
                  onchange={saveSettings}
                />
              </label>
            </div>

            <button class="ghost-button" onclick={clearLoginInfo}>
              Forget login
            </button>
          </section>

          <section class="settings-card">
            <div>
              <h2>Downloads & library</h2>
              <p>
                Albums download to <code>_pending</code> first, then move into your
                library folder when complete.
              </p>
            </div>

            <div class="folder-picker">
              <input
                readonly
                placeholder="No folder selected"
                value={settings.downloadFolder}
              />

              <button onclick={chooseDownloadFolder}>Choose folder</button>
            </div>
          </section>

          <section class="settings-card">
            <div>
              <h2>Search</h2>
              <p>Longer searches can return more results, but feel slower.</p>
            </div>

            <label class="setting-row">
              <span>Search timeout</span>
              <select
                bind:value={settings.searchTimeoutSeconds}
                onchange={saveSettings}
              >
                <option value={5}>5 seconds</option>
                <option value={10}>10 seconds</option>
                <option value={15}>15 seconds</option>
                <option value={20}>20 seconds</option>
              </select>
            </label>
          </section>
        </div>
      {:else if downloadGroups.length}
        <div class="downloads-page">
          {#each downloadGroups as group}
            <div class="download-folder">
              <div class="download-folder-header">
                <div>
                  <strong>{group.packageName}</strong>
                  <span>
                    {group.items.length} tracks · {formatBytes(group.totalSize)}
                    · {group.status}
                    {#if group.status === "downloading" && formatSpeed(getGroupSpeed(group))}
                      · {formatSpeed(getGroupSpeed(group))}
                      . {formatEta(getGroupEta(group))}
                    {/if}
                  </span>
                </div>

                <div class="download-folder-actions">
                  <small>{Math.round(group.progress)}%</small>

                  <button
                    class="icon-action"
                    title={group.status === "paused"
                      ? "Resume album"
                      : "Pause album"}
                    onclick={() => pauseDownloadGroup(group.packageId)}
                  >
                    {group.status === "paused" ? "▶" : "Ⅱ"}
                  </button>

                  <button
                    class="icon-action danger"
                    title="Delete album"
                    onclick={() => deleteDownloadGroup(group.packageId)}
                  >
                    ×
                  </button>
                </div>
              </div>

              <div class="download-progress">
                <div style={`width: ${group.progress}%`}></div>
              </div>

              <div class="download-track-list">
                {#each group.items as item, index}
                  <div class="download-row">
                    <div class="download-main">
                      <strong>{item.name}</strong>
                      <span>
                        {item.username} · {formatBytes(item.size)}
                        {#if item.attributes}
                          · {item.attributes}
                        {/if}
                        {#if item.duration_seconds}
                          · {formatDuration(item.duration_seconds)}
                        {/if}
                        {#if item.error}
                          · {item.error}
                        {/if}
                      </span>
                    </div>

                    <div class="download-status">
                      <span>{item.status}</span>
                      <small>
                        {Math.round(item.progress)}%
                        {#if item.status === "downloading" && formatSpeed(item.speed_bytes_per_sec)}
                          · {formatSpeed(item.speed_bytes_per_sec)}
                        {/if}
                      </small>
                      <small class="download-eta"
                        >{formatEta(getItemEta(item))}</small
                      >
                    </div>

                    <div class="download-actions">
                      <button
                        class="icon-action"
                        title="Move up"
                        onclick={() => moveDownload(item.id, -1)}
                        disabled={index === 0}
                      >
                        ↑
                      </button>

                      <button
                        class="icon-action"
                        title="Move down"
                        onclick={() => moveDownload(item.id, 1)}
                        disabled={index === group.items.length - 1}
                      >
                        ↓
                      </button>

                      <button
                        class="icon-action"
                        title={item.status === "paused" ? "Resume" : "Pause"}
                        onclick={() => pauseDownload(item.id)}
                      >
                        {item.status === "paused" ? "▶" : "Ⅱ"}
                      </button>

                      <button
                        class="icon-action danger"
                        title="Delete"
                        onclick={() => deleteDownload(item.id)}
                      >
                        ×
                      </button>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty-state">
          <h2>No active downloads</h2>
          <p>Right-click a folder or track in Search to queue downloads.</p>
        </div>
      {/if}
    </main>

    <footer
      class="player"
      class:has-art={!!getCurrentTrackCover()}
      style={`--player-art: url("${getCurrentTrackCover() ?? ""}");`}
    >
      <audio
        bind:this={audioRef}
        src={audioSrc || undefined}
        preload="metadata"
        ontimeupdate={onAudioTimeUpdate}
        onloadedmetadata={onAudioLoadedMetadata}
        onplay={() => (isPlaying = true)}
        onpause={() => (isPlaying = false)}
        onended={onAudioEnded}
        onerror={() => {
          console.error("audio failed", {
            path: currentTrack?.path,
            extension: currentTrack?.extension,
            srcStart: audioRef?.src?.slice(0, 80),
            errorCode: audioRef?.error?.code,
            errorMessage: audioRef?.error?.message,
            canPlayM4a: audioRef?.canPlayType("audio/mp4"),
            canPlayXMusicM4a: audioRef?.canPlayType("audio/x-m4a"),
            canPlayAac: audioRef?.canPlayType("audio/aac"),
          });
        }}
      ></audio>

      <div class="player-bg"></div>

      <div class="track">
        <div>
          <strong>{getTrackTitle(currentTrack)}</strong>
          <span>{getTrackSubtitle(currentTrack)}</span>
        </div>
      </div>

      <div class="player-progress-wrap">
        <span>{formatPlayerTime(playerTime)}</span>

        <button class="bar" onclick={seekPlayer} aria-label="Seek">
          <div style={`width: ${playerProgressPercent}%`}></div>
        </button>

        <span>{formatPlayerTime(playerDuration)}</span>
      </div>

      <div class="transport">
        <button aria-label="Previous" onclick={() => playAdjacentTrack(-1)}>
          <Rewind size={19} weight="regular" />
        </button>

        <button class="play" aria-label="Play" onclick={togglePlayer}>
          {#if isPlaying}
            <Pause size={22} weight="fill" />
          {:else}
            <Play size={22} weight="fill" />
          {/if}
        </button>

        <button aria-label="Next" onclick={() => playAdjacentTrack(1)}>
          <FastForward size={19} weight="regular" />
        </button>
      </div>
    </footer>
  </section>
</div>

{#if contextMenuOpen}
  <div
    class="context-menu"
    style={`left: ${contextMenuX}px; top: ${contextMenuY}px;`}
    onclick={(e) => e.stopPropagation()}
  >
    {#each contextMenuItems as item}
      <button
        class:disabled={item.disabled}
        onclick={() => runContextAction(item)}
      >
        {item.label}
      </button>
    {/each}
  </div>
{/if}
{#if editingAlbum}
  <div class="modal-backdrop" onclick={closeEditAlbum}>
    <div class="edit-album-modal" onclick={(event) => event.stopPropagation()}>
      <div class="modal-header">
        <div>
          <strong>{editingAlbum.isSingle ? "Edit song" : "Edit album"}</strong>
          <span>{editingAlbum.folder}</span>
        </div>

        <button onclick={closeEditAlbum}>×</button>
      </div>

      <label class="edit-field">
        <span>Album name</span>
        <input bind:value={editAlbumName} />
      </label>

      <div class="edit-track-list">
        {#each editingAlbum.tracks as track}
          <label class="edit-field track-edit-field">
            <span>{track.name}</span>
            <input bind:value={editTrackNames[track.id]} />
          </label>
        {/each}
      </div>

      <div class="modal-actions">
        <button onclick={closeEditAlbum}>Cancel</button>
        <button class="primary" onclick={saveEditedAlbum}>Save changes</button>
      </div>
    </div>
  </div>
{/if}
