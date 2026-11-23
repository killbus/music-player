import _ from "lodash";
import { useIsMutating, useQueryClient } from "@tanstack/react-query";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import { fetcher } from "../Api/fetcher";
import { resourceUriResolver } from "../ResourceUriResolver";
import { nowPlayingAtom, playbackControlErrorAtom, playbackIndexAtom, playbackPositionAtom } from "../State";
import { Track } from "../Types";
import {
  PauseDocument,
  PauseMutation,
  PlayDocument,
  PlayMutation,
  PlayAlbumMutationVariables,
  PlayArtistTracksMutationVariables,
  PlayNextMutationVariables,
  PlayPlaylistMutationVariables,
  PlayTrackAtMutationVariables,
  RemoveTrackAtMutationVariables,
  useCurrentlyPlayingSongQuery,
  useGetTracklistQuery,
  useNextMutation,
  usePauseMutation,
  usePlayAlbumMutation,
  usePlayArtistTracksMutation,
  usePlayMutation,
  usePlayNextMutation,
  usePlayPlaylistMutation,
  usePlayTrackAtMutation,
  usePreviousMutation,
  useRemoveTrackAtMutation,
  useSeekMutation,
} from "./GraphQL";

const toTrack = (track: any): Track => ({
  id: track.id,
  artist: track.artists.map((artist: any) => artist.name).join(", "),
  title: track.title,
  album: track.album.title,
  duration: track.duration!,
  artistId: _.get(track, "artists.0.id", ""),
  cover:
    track.album.cover && !_.startsWith(track.album.cover, "https://")
      ? resourceUriResolver.resolve(`/covers/${track.album.cover}`)
      : track.album.cover!,
});

const controlKey = ["PlaybackControl"] as const;

async function sendControl<T>(document: string, field: keyof T): Promise<T> {
  const signal = AbortSignal.timeout(10000);
  try {
    const result = await fetcher<T, Record<string, never>>(document, {}, undefined, signal)();
    if (!result[field]) throw new Error("The player did not accept the playback command.");
    return result;
  } catch (error) {
    if (signal.aborted) throw new Error("Playback command timed out; outcome uncertain. Check playback status before retrying.");
    throw error;
  }
}

export const usePlayback = () => {
  const nowPlaying = useAtomValue(nowPlayingAtom);
  const index = useAtomValue(playbackIndexAtom);
  const [controlError, setControlError] = useAtom(playbackControlErrorAtom);
  const controlPending = useIsMutating({ mutationKey: controlKey }) > 0;
  const setPosition = useSetAtom(playbackPositionAtom);
  const queryClient = useQueryClient();

  const invalidatePlayback = () => {
    queryClient.invalidateQueries({
      queryKey: useCurrentlyPlayingSongQuery.getKey(),
    });
    queryClient.invalidateQueries({
      queryKey: useGetTracklistQuery.getKey(),
    });
    queryClient.invalidateQueries({ queryKey: ["SourcePlayback"] });
  };

  const controlOptions = {
    mutationKey: controlKey,
    retry: false,
    onMutate: () => setControlError(null),
    onError: (error: Error) => setControlError(error.message),
    onSettled: invalidatePlayback,
  };
  const playMutation = usePlayMutation<Error>({
    ...controlOptions,
    mutationFn: () => sendControl<PlayMutation>(PlayDocument, "play"),
  });
  const pauseMutation = usePauseMutation<Error>({
    ...controlOptions,
    mutationFn: () => sendControl<PauseMutation>(PauseDocument, "pause"),
  });
  const nextMutation = useNextMutation({ onSuccess: invalidatePlayback });
  const previousMutation = usePreviousMutation({
    onSuccess: invalidatePlayback,
  });
  const playTrackAtMutation = usePlayTrackAtMutation({
    onSuccess: invalidatePlayback,
  });
  const removeTrackAtMutation = useRemoveTrackAtMutation({
    onSuccess: invalidatePlayback,
  });
  const playNextMutation = usePlayNextMutation({
    onSuccess: invalidatePlayback,
  });
  const playAlbumMutation = usePlayAlbumMutation({
    onSuccess: invalidatePlayback,
  });
  const playArtistTracksMutation = usePlayArtistTracksMutation({
    onSuccess: invalidatePlayback,
  });
  const playPlaylistMutation = usePlayPlaylistMutation({
    onSuccess: invalidatePlayback,
  });
  const seekMutation = useSeekMutation();

  const { data: queue } = useGetTracklistQuery(undefined, {
    refetchInterval: 5000,
  });

  const nextTracks: Track[] =
    queue?.tracklistTracks?.nextTracks?.map(toTrack) || [];
  const previousTracks: Track[] =
    queue?.tracklistTracks?.previousTracks?.map(toTrack) || [];

  const play = () => {
    // Acceptance only enqueues a command. Playback is confirmed by the daemon.
    if (!queryClient.isMutating({ mutationKey: controlKey })) playMutation.mutate({});
  };

  const pause = () => {
    if (!queryClient.isMutating({ mutationKey: controlKey })) pauseMutation.mutate({});
  };

  const seek = (positionMs: number) => {
    // optimistic update of the position, the trackTimePosition
    // subscription will keep it in sync afterwards
    setPosition(positionMs);
    return seekMutation.mutateAsync({ position: positionMs });
  };

  return {
    nowPlaying,
    index,
    nextTracks,
    previousTracks,
    controlError,
    controlPending,
    play,
    pause,
    seek,
    next: () => nextMutation.mutateAsync({}),
    previous: () => previousMutation.mutateAsync({}),
    playTrackAt: (variables: PlayTrackAtMutationVariables) =>
      playTrackAtMutation.mutateAsync(variables),
    removeTrackAt: (variables: RemoveTrackAtMutationVariables) =>
      removeTrackAtMutation.mutateAsync(variables),
    playNext: (variables: PlayNextMutationVariables) =>
      playNextMutation.mutateAsync(variables),
    playAlbum: (variables: PlayAlbumMutationVariables) =>
      playAlbumMutation.mutateAsync(variables),
    playArtistTracks: (variables: PlayArtistTracksMutationVariables) =>
      playArtistTracksMutation.mutateAsync(variables),
    playPlaylist: (variables: PlayPlaylistMutationVariables) =>
      playPlaylistMutation.mutateAsync(variables),
  };
};
