import { useQuery } from "@tanstack/react-query";
import { busyPhases } from "../app/trackUtils";
import { api } from "./client";
import type { Me, MusicRequest, RecentSong, Setup, Track, WantedSong, Workflow } from "./types";

export const queryKeys = {
  me: ["me"] as const,
  setup: ["setup"] as const,
  status: ["status"] as const,
  tracks: ["tracks"] as const,
  requests: ["requests"] as const,
  recent: ["library", "recent"] as const,
};

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
  });
}

export function useStatusQuery() {
  return useQuery({
    queryKey: queryKeys.status,
    queryFn: () => api<Workflow>("/status"),
    refetchInterval: (query) => {
      const phase = query.state.data?.phase;
      return phase && busyPhases.has(phase) ? 900 : false;
    },
    refetchIntervalInBackground: true,
  });
}

export function useTracksQuery(busy: boolean) {
  return useQuery({
    queryKey: queryKeys.tracks,
    queryFn: () => api<Track[]>("/tracks"),
    refetchInterval: busy ? 5400 : false,
    refetchIntervalInBackground: true,
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

export function useRecentSongsQuery() {
  return useQuery({
    queryKey: queryKeys.recent,
    queryFn: () => api<RecentSong[]>("/library/recent"),
    refetchInterval: 15000,
  });
}
