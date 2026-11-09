import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAtomValue } from "jotai";
import { fetcher } from "../Api/fetcher";
import { currentCastDeviceAtom } from "../State";
import type { PlaybackModes as ApiPlaybackModes, RepeatMode as ApiRepeatMode } from "./GraphQL";

export type RepeatMode = `${ApiRepeatMode}`;
export type PlaybackModes = Omit<ApiPlaybackModes, "__typename" | "repeat"> & { repeat: RepeatMode | null };
export const playbackModesKey = ["PlaybackModes"] as const;
const mutationKey = ["SetPlaybackModes"] as const;
const nextRepeat = { OFF: "ALL", ALL: "ONE", ONE: "OFF" } as const;

export function usePlaybackModes() {
  const client = useQueryClient();
  const receiver = useAtomValue(currentCastDeviceAtom);
  const query = useQuery({
    queryKey: [...playbackModesKey, receiver?.id ?? "local"],
    queryFn: ({ signal }) => fetcher<{ playbackModes: PlaybackModes }, Record<string, never>>(
      `query PlaybackModes { playbackModes { available unavailableReason shuffle repeat } }`,
      {}, undefined, AbortSignal.any([signal, AbortSignal.timeout(10000)])
    )(),
    refetchInterval: 2000,
    retry: false,
  });
  const mutation = useMutation({
    mutationKey,
    retry: false,
    mutationFn: async (change: { shuffle: boolean } | { repeat: RepeatMode }) => {
      // Abort old reads so a delayed pre-command response cannot overwrite state.
      await client.cancelQueries({ queryKey: playbackModesKey });
      const signal = AbortSignal.timeout(10000);
      try {
        const result = "shuffle" in change
          ? await fetcher<{ setShuffle: boolean }, { enabled: boolean }>(
            `mutation SetShuffle($enabled: Boolean!) { setShuffle(enabled: $enabled) }`,
            { enabled: change.shuffle }, undefined, signal
          )()
          : await fetcher<{ setRepeat: boolean }, { mode: RepeatMode }>(
            `mutation SetRepeat($mode: RepeatMode!) { setRepeat(mode: $mode) }`,
            { mode: change.repeat }, undefined, signal
          )();
        if (!("setShuffle" in result ? result.setShuffle : result.setRepeat)) {
          throw new Error("The player did not accept the mode change.");
        }
      } catch (error) {
        if (signal.aborted) throw new Error("Mode request timed out; outcome uncertain. Refresh to check the current mode.");
        throw error;
      }
    },
    onSettled: async () => {
      await client.cancelQueries({ queryKey: playbackModesKey });
      void client.invalidateQueries({ queryKey: ["GetTracklist"] });
      void client.invalidateQueries({ queryKey: ["MediaQueue"] });
      await client.invalidateQueries({ queryKey: playbackModesKey });
    },
  });
  const pending = useIsMutating({ mutationKey }) > 0;
  const state = query.data?.playbackModes;
  let unavailable: string | null = null;
  if (receiver) unavailable = "Playback modes are unavailable on this remote receiver.";
  else if (query.isError) unavailable = "Unable to read playback modes. Refresh to try again.";
  else if (!state) unavailable = "Loading playback modes...";
  else if (!state.available) unavailable = state.unavailableReason ?? "Playback modes are unavailable.";
  const disabled = pending || !!unavailable || state?.shuffle == null || state?.repeat == null;
  const change = (value: { shuffle: boolean } | { repeat: RepeatMode }) => {
    // Also covers two surfaces clicked before React has rendered pending state.
    if (disabled || client.isMutating({ mutationKey })) return;
    mutation.mutate(value);
  };
  return {
    shuffle: unavailable ? null : state?.shuffle ?? null,
    repeat: unavailable ? null : state?.repeat ?? null,
    disabled,
    pending,
    message: pending ? "Updating playback mode..." : unavailable,
    error: mutation.error?.message ?? null,
    refresh: () => { mutation.reset(); void query.refetch(); },
    toggleShuffle: () => change({ shuffle: !state?.shuffle }),
    cycleRepeat: () => { if (state?.repeat) change({ repeat: nextRepeat[state.repeat] }); },
  };
}

export type PlaybackModeControlsState = ReturnType<typeof usePlaybackModes>;
