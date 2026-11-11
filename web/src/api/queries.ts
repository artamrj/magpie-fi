import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { busyPhases } from "../app/trackUtils";
import { api } from "./client";
import type {
  ArtistSummary,
  FolderTags,
  LibraryFolder,
  LibrarySearch,
  Me,
  MusicRequest,
  Setup,
  SongDetails,
  SplitAlbum,
  Track,
  TrackList,
  WantedSong,
  Workflow,
} from "./types";

export const queryKeys = {
  me: ["me"] as const,
  setup: ["setup"] as const,
  status: ["status"] as const,
  tracks: ["tracks"] as const,
  requests: ["requests"] as const,
  artists: ["artists"] as const,
  files: ["files"] as const,
};

export function useArtistsQuery() {
  return useQuery({
    queryKey: queryKeys.artists,
    queryFn: () => api<ArtistSummary[]>("/artists"),
    refetchInterval: 30000,
  });
}

/** The artist's albums that Navidrome shows twice. Reads every song's tags, so only on demand. */
export function useSplitAlbumsQuery(artistId: number) {
  return useQuery({
    queryKey: [...queryKeys.artists, artistId, "albums"],
    queryFn: () => api<SplitAlbum[]>(`/artists/${artistId}/albums`),
  });
}

export function useMeQuery() {
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: () => api<Me>("/me"),
    staleTime: Infinity,
  });
}

export function useSetupQuery() {
  return useQuery({
    queryKey: queryKeys.setup,
    queryFn: () => api<Setup>("/setup"),
    refetchInterval: 30000,
  });
}

/** The server's state. Songs arrive and get filed without the page's help, so it always polls:
 *  fast while the server works, slowly while it waits. */
export function useStatusQuery() {
  return useQuery({
    queryKey: queryKeys.status,
    queryFn: () => api<Workflow>("/status"),
    refetchInterval: (query) => {
      const phase = query.state.data?.phase;
      return phase && busyPhases.has(phase) ? 900 : 4000;
    },
    refetchIntervalInBackground: true,
  });
}

/** Songs of each Review section the list shows before "Show more". */
export const REVIEW_PAGE = 50;

/** The Review list: the counts of every section and the first `limit` songs of each that fit
 *  `search`. The answer stays small however many songs wait, so it can be asked for often. */
export function useTracksQuery(busy: boolean, search: string, limit: number) {
  const params = new URLSearchParams({ limit: String(limit) });
  if (search) params.set("q", search);
  return useQuery({
    queryKey: [...queryKeys.tracks, "list", search, limit],
    queryFn: () => api<TrackList>(`/tracks?${params.toString()}`),
    refetchInterval: busy ? 3000 : 10000,
    refetchIntervalInBackground: true,
    // While typing or showing more, the old rows stay until the new ones arrive.
    placeholderData: keepPreviousData,
  });
}

/** One song with every match found for it, fetched when the song is opened. */
export function useTrackQuery(id: number | undefined, busy: boolean) {
  return useQuery({
    queryKey: [...queryKeys.tracks, "one", id],
    queryFn: () => api<Track>(`/tracks/${id}`),
    enabled: id !== undefined,
    refetchInterval: busy ? 3000 : 10000,
    // A song that is gone (filed or removed) is not asked for again.
    retry: false,
  });
}

/** A request is still moving while its link is read or songs are on the way. */
export const isActiveRequest = (request: MusicRequest) =>
  request.status === "reading" || request.working > 0;

export function useRequestsQuery() {
  return useQuery({
    queryKey: queryKeys.requests,
    queryFn: () => api<MusicRequest[]>("/requests"),
    refetchInterval: (query) => (query.state.data?.some(isActiveRequest) ? 2000 : 15000),
    refetchIntervalInBackground: true,
  });
}

export function useRequestSongsQuery(request: MusicRequest) {
  return useQuery({
    queryKey: [...queryKeys.requests, request.id],
    queryFn: () => api<WantedSong[]>(`/requests/${request.id}`),
    refetchInterval: isActiveRequest(request) ? 3000 : false,
  });
}

const pathParam = (path: string) => encodeURIComponent(path);

/** One folder of the library ("" is the library itself). */
export function useFolderQuery(path: string, enabled = true) {
  return useQuery({
    queryKey: [...queryKeys.files, "folder", path],
    queryFn: () => api<LibraryFolder>(`/files?path=${pathParam(path)}`),
    enabled,
    staleTime: 10000,
  });
}

/** Songs and folders matching every word; keeps the last results while typing. */
export function useFileSearchQuery(query: string, within: string) {
  return useQuery({
    queryKey: [...queryKeys.files, "search", query, within],
    queryFn: () =>
      api<LibrarySearch>(`/files/search?q=${encodeURIComponent(query)}&in=${pathParam(within)}`),
    enabled: query.length > 0,
    placeholderData: (previous) => previous,
    staleTime: 10000,
  });
}

export function useSongDetailsQuery(path: string) {
  return useQuery({
    queryKey: [...queryKeys.files, "song", path],
    queryFn: () => api<SongDetails>(`/files/song?path=${pathParam(path)}`),
  });
}

/** What the songs directly in a folder say, for "Edit all songs". Read fresh each time. */
export function useFolderTagsQuery(path: string) {
  return useQuery({
    queryKey: [...queryKeys.files, "folder-tags", path],
    queryFn: () => api<FolderTags>(`/files/folder-tags?path=${pathParam(path)}`),
    staleTime: 0,
  });
}
