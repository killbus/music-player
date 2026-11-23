import { QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { graphql, HttpResponse } from "msw";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { nowPlayingAtom } from "../State";
import { makeQueryClient } from "../test/render";
import { server } from "../test/server";
import { PlayerStateChangedDocument, useCurrentlyPlayingSongQuery } from "./GraphQL";
import { usePlayback } from "./usePlayback";
import { usePlaybackSync } from "./usePlaybackSync";
import { useSourcePlayback } from "./useSourcePlayback";

const subscriptions = vi.hoisted(() => new Map<string, (data: any) => void>());
vi.mock("./useGraphQLSubscription", () => ({
  useGraphQLSubscription: (document: string, _variables: unknown, callback: (data: any) => void) => {
    subscriptions.set(document, callback);
    return {};
  },
}));

function setup(isPlaying = false) {
  const store = createStore();
  const client = makeQueryClient();
  const state = { track: null, index: 0, positionMs: 12345, isPlaying };
  server.use(graphql.query("CurrentlyPlayingSong", () =>
    HttpResponse.json({ data: { currentlyPlayingSong: { ...state } } })
  ));
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>
      <Provider store={store}>{children}</Provider>
    </QueryClientProvider>
  );
  const hook = renderHook(() => { usePlaybackSync(); return usePlayback(); }, { wrapper });
  return { ...hook, store, client, state };
}

describe("playback controls and authoritative synchronization", () => {
  it("keeps Play and the checkpoint when an accepted command produces no playback", async () => {
    let plays = 0;
    server.use(graphql.mutation("Play", () => {
      plays++;
      return HttpResponse.json({ data: { play: true } });
    }));
    const { result, store, client } = setup();
    await waitFor(() => expect(store.get(nowPlayingAtom).progress).toBe(12345));
    const before = client.getQueryData(useCurrentlyPlayingSongQuery.getKey());
    act(() => { result.current.play(); });
    await waitFor(() => expect(plays).toBe(1));
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(client.getQueryData(useCurrentlyPlayingSongQuery.getKey())).toBe(before);
    expect(store.get(nowPlayingAtom).isPlaying).toBe(false);
    expect(store.get(nowPlayingAtom).progress).toBe(12345);
    act(() => { result.current.play(); });
    await waitFor(() => expect(plays).toBe(2));
  });

  it("does not claim a pause until the daemon reports it", async () => {
    server.use(graphql.mutation("Pause", () => HttpResponse.json({ data: { pause: true } })));
    const { result, store, client, state } = setup(true);
    await waitFor(() => expect(store.get(nowPlayingAtom).isPlaying).toBe(true));
    act(() => { result.current.pause(); });
    await waitFor(() => expect(client.isMutating()).toBe(0));
    expect(store.get(nowPlayingAtom).isPlaying).toBe(true);
    state.isPlaying = false;
    await act(() => client.invalidateQueries({ queryKey: useCurrentlyPlayingSongQuery.getKey() }));
    await waitFor(() => expect(store.get(nowPlayingAtom).isPlaying).toBe(false));
  });

  it("repairs subscription drift even when poll data is structurally identical", async () => {
    const { store, client } = setup();
    await waitFor(() => expect(store.get(nowPlayingAtom).isPlaying).toBe(false));
    const before = client.getQueryData(useCurrentlyPlayingSongQuery.getKey());
    act(() => store.set(nowPlayingAtom, { ...store.get(nowPlayingAtom), isPlaying: true }));
    await act(() => client.invalidateQueries({ queryKey: useCurrentlyPlayingSongQuery.getKey() }));
    expect(client.getQueryData(useCurrentlyPlayingSongQuery.getKey())).toBe(before);
    await waitFor(() => expect(store.get(nowPlayingAtom).isPlaying).toBe(false));
  });

  it("reports command rejection and leaves playback available for retry", async () => {
    server.use(graphql.mutation("Play", () => HttpResponse.json({ errors: [{ message: "Output unavailable" }] })));
    const { result, store } = setup();
    await waitFor(() => expect(store.get(nowPlayingAtom).isPlaying).toBe(false));
    act(() => { result.current.play(); });
    await waitFor(() => expect(result.current.controlError).toBe("Output unavailable"));
    expect(store.get(nowPlayingAtom).isPlaying).toBe(false);
  });

  it("refreshes startup status on a playback event without waiting for a polling interval", async () => {
    const source = "mp-source:fixture";
    let phase = "OPENING";
    server.use(graphql.query("SourcePlayback", () => HttpResponse.json({ data: {
      sourcePlayback: { source, occurrenceId: "fixture", phase, error: null },
    } })));
    const { client, store } = setup();
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}><Provider store={store}>{children}</Provider></QueryClientProvider>
    );
    const status = renderHook(() => useSourcePlayback(source), { wrapper });
    await waitFor(() => expect(status.result.current.starting).toBe(true));
    phase = "STREAMING";
    act(() => subscriptions.get(PlayerStateChangedDocument)!({ playerState: { isPlaying: true } }));
    await waitFor(() => expect(status.result.current.starting).toBe(false));
  });
});
