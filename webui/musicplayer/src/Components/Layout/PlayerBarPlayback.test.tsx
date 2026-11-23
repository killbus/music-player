import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { graphql, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { currentCastDeviceAtom, nowPlayingAtom } from "../../State";
import { makeQueryClient } from "../../test/render";
import { server } from "../../test/server";
import PlayerBarWithData from "./PlayerBarWithData";

vi.mock("../../Hooks/useLevels", () => ({ useLevels: () => ({ left: 0, right: 0 }) }));
vi.mock("../../Hooks/useLikes", () => ({ useLikes: () => ({ isLiked: () => false, toggleLike: vi.fn() }) }));
vi.mock("../../Hooks/useVolume", () => ({ useVolume: () => ({ volume: 1, muted: false, change: vi.fn(), toggleMute: vi.fn() }) }));
vi.mock("../../Hooks/useDevices", () => ({ useDevices: () => ({}) }));

const source = "mp-source:v1?resolver=emby&account=a&server=s&user=u&kind=item&id=episode";
function setup(remote = false) {
  const store = createStore();
  store.set(nowPlayingAtom, { id: source, title: "Episode", progress: 12345, duration: 60000, isPlaying: false });
  if (remote) store.set(currentCastDeviceAtom, { id: "receiver", name: "Receiver" } as any);
  const client = makeQueryClient();
  render(<QueryClientProvider client={client}><Provider store={store}>
    <PlayerBarWithData onOpenDevices={() => {}} />
  </Provider></QueryClientProvider>);
  return { store, client };
}

function status(phase: string, error: string | null = null, item = source) {
  server.use(graphql.query("SourcePlayback", () => HttpResponse.json({ data: {
    sourcePlayback: { source: item, occurrenceId: "occurrence", phase, error, retryAttempt: 1, retryLimit: 2 },
  } })));
}

describe("managed playback feedback", () => {
  it("shows a failed stream and retries Play without changing the checkpoint", async () => {
    status("FAILED", "Audio server returned HTTP 403. Press Play to resolve a fresh stream.");
    const play = vi.fn();
    server.use(graphql.mutation("Play", () => { play(); return HttpResponse.json({ data: { play: true } }); }));
    const { store } = setup();
    expect(await screen.findByRole("alert")).toHaveTextContent("HTTP 403");
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    await waitFor(() => expect(play).toHaveBeenCalledOnce());
    await waitFor(() => expect(screen.getByRole("button", { name: "Play" })).toBeEnabled());
    expect(store.get(nowPlayingAtom).isPlaying).toBe(false);
    expect(store.get(nowPlayingAtom).progress).toBe(12345);
  });

  it.each(["RESOLVING", "OPENING", "RECOVERING"])("can cancel %s without pretending that audio is playing", async (phase) => {
    status(phase);
    const pause = vi.fn();
    server.use(graphql.mutation("Pause", () => { pause(); return HttpResponse.json({ data: { pause: true } }); }));
    const { store } = setup();
    if (phase === "RECOVERING") {
      expect(await screen.findByText("Recovering playback… (1/2)")).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    }
    fireEvent.click(await screen.findByRole("button", { name: "Cancel playback startup" }));
    await waitFor(() => expect(pause).toHaveBeenCalledOnce());
    expect(store.get(nowPlayingAtom).isPlaying).toBe(false);
  });

  it("shows a rejected playback command", async () => {
    status("PAUSED");
    server.use(graphql.mutation("Play", () => HttpResponse.json({ errors: [{ message: "Local output failed" }] })));
    setup();
    fireEvent.click(screen.getByRole("button", { name: "Play" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Local output failed");
    await waitFor(() => expect(screen.getByRole("button", { name: "Play" })).toBeEnabled());
  });

  it("discards another item's status", async () => {
    status("FAILED", "stale failure", "another-source");
    const { client } = setup();
    await act(() => client.invalidateQueries({ queryKey: ["SourcePlayback"] }));
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(screen.queryByText("stale failure")).not.toBeInTheDocument();
  });

  it("does not apply local source status to a cast receiver", async () => {
    const read = vi.fn();
    server.use(graphql.query("SourcePlayback", () => { read(); return HttpResponse.json({ data: { sourcePlayback: null } }); }));
    const { client } = setup(true);
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(read).not.toHaveBeenCalled();
  });
});
