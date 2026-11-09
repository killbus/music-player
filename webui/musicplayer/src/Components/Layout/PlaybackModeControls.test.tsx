import { QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createStore, Provider } from "jotai";
import { graphql, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import type { PlaybackModes } from "../../Hooks/usePlaybackModes";
import { currentCastDeviceAtom, nowPlayingAtom, queueOpenAtom } from "../../State";
import type { Device } from "../../Types/Device";
import { makeQueryClient } from "../../test/render";
import { server } from "../../test/server";
import PlayerBarWithData from "./PlayerBarWithData";
import QueueDrawer from "./QueueDrawer";

// Keep the real mode hook, data connector and controls; unrelated meters/likes
// do not participate in this API contract.
vi.mock("../../Hooks/useLevels", () => ({ useLevels: () => ({ left: 0, right: 0 }) }));
vi.mock("../../Hooks/useLikes", () => ({ useLikes: () => ({ isLiked: () => false, toggleLike: vi.fn() }) }));
vi.mock("../../Hooks/useVolume", () => ({ useVolume: () => ({ volume: 1, muted: false, change: vi.fn(), toggleMute: vi.fn() }) }));

function setup(queue: boolean | "both" = false) {
  const store = createStore();
  store.set(nowPlayingAtom, {
    id: "mp-source:v1?resolver=emby&account=a&server=s&user=u&kind=item&id=episode",
    title: "Emby episode", artist: "", album: "Season", progress: 500, duration: 2000, isPlaying: true,
  });
  store.set(queueOpenAtom, !!queue);
  const client = makeQueryClient();
  const view = render(<QueryClientProvider client={client}><Provider store={store}>
    {queue && <QueueDrawer />}
    {queue !== true && <PlayerBarWithData onOpenDevices={() => {}} />}
  </Provider></QueryClientProvider>);
  return { ...view, store, client };
}

function api(initial: Partial<PlaybackModes> = {}) {
  const state: PlaybackModes = { available: true, unavailableReason: null, shuffle: false, repeat: "OFF", ...initial };
  const writes = vi.fn();
  server.use(
    graphql.query("PlaybackModes", () => HttpResponse.json({ data: { playbackModes: { ...state } } })),
    graphql.mutation("SetShuffle", ({ variables }) => {
      writes("shuffle", variables.enabled);
      state.shuffle = variables.enabled;
      return HttpResponse.json({ data: { setShuffle: true } });
    }),
    graphql.mutation("SetRepeat", ({ variables }) => {
      writes("repeat", variables.mode);
      state.repeat = variables.mode;
      return HttpResponse.json({ data: { setRepeat: true } });
    }),
  );
  return { state, writes };
}

describe("data-connected playback modes", () => {
  it("controls Emby shuffle and cycles off/all/one without replacing the current item", async () => {
    const { writes } = api();
    const { store } = setup();
    const before = store.get(nowPlayingAtom);
    await waitFor(() => expect(screen.getByRole("button", { name: "Shuffle: off" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Shuffle: off" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Shuffle: on" })).toBeEnabled());
    expect(screen.getByRole("button", { name: "Shuffle: on" })).toHaveAttribute("aria-pressed", "true");
    for (const [from, to] of [["off", "all"], ["all", "one"], ["one", "off"]]) {
      fireEvent.click(screen.getByRole("button", { name: `Repeat: ${from}` }));
      await waitFor(() => expect(screen.getByRole("button", { name: `Repeat: ${to}` })).toBeEnabled());
      if (to === "one") expect(screen.getByText("1")).toBeInTheDocument();
    }
    expect(writes.mock.calls).toEqual([["shuffle", true], ["repeat", "ALL"], ["repeat", "ONE"], ["repeat", "OFF"]]);
    expect(store.get(nowPlayingAtom)).toEqual(before);
  });

  it("has visible mode controls in the mobile queue surface and reads another client's changes", async () => {
    const { state } = api();
    setup(true);
    const panel = screen.getByRole("region", { name: "Playback modes" });
    await waitFor(() => expect(within(panel).getByRole("button", { name: "Repeat: off" })).toBeEnabled());
    expect(panel).not.toHaveClass("hidden");
    expect(within(panel).getByRole("button", { name: "Shuffle: off" })).not.toHaveClass("hidden");
    state.shuffle = true;
    state.repeat = "ONE";
    await waitFor(() => expect(within(panel).getByRole("button", { name: "Repeat: one" })).toHaveAttribute("aria-pressed", "true"), { timeout: 4000 });
    fireEvent.click(within(panel).getByRole("button", { name: "Shuffle: on" }));
    await waitFor(() => expect(within(panel).getByRole("button", { name: "Shuffle: off" })).toBeEnabled());
  });

  it("does not optimistically claim success or send duplicate requests while pending", async () => {
    api();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const writes = vi.fn();
    server.use(graphql.mutation("SetShuffle", async () => {
      writes(); await gate;
      return HttpResponse.json({ errors: [{ message: "Player command channel closed" }] });
    }));
    setup();
    const button = await screen.findByRole("button", { name: "Shuffle: off" });
    fireEvent.click(button);
    await waitFor(() => expect(button).toBeDisabled());
    fireEvent.click(button);
    expect(button).toHaveAttribute("aria-pressed", "false");
    await act(async () => { release(); });
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("Player command channel closed"));
    await waitFor(() => expect(button).toBeEnabled());
    expect(writes).toHaveBeenCalledTimes(1);
  });

  it("shares acknowledged state and pending guards between the bar and queue", async () => {
    const { writes } = api();
    setup("both");
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Shuffle: off" })).toHaveLength(2));
    const buttons = screen.getAllByRole("button", { name: "Shuffle: off" });
    fireEvent.click(buttons[0]);
    fireEvent.click(buttons[1]);
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Shuffle: on" })).toHaveLength(2));
    expect(writes).toHaveBeenCalledTimes(1);
  });

  it("releases pending state after a network timeout without claiming success", async () => {
    api();
    const deadlines: AbortController[] = [];
    const timeout = vi.spyOn(AbortSignal, "timeout").mockImplementation(() => {
      const controller = new AbortController();
      deadlines.push(controller);
      return controller.signal;
    });
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const started = vi.fn();
    server.use(graphql.mutation("SetShuffle", async () => {
      started(); await gate;
      return HttpResponse.json({ data: { setShuffle: true } });
    }));
    const view = setup();
    try {
      fireEvent.click(await screen.findByRole("button", { name: "Shuffle: off" }));
      await waitFor(() => expect(started).toHaveBeenCalledTimes(1));
      act(() => deadlines.at(-1)!.abort());
      await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("outcome uncertain"));
      await waitFor(() => expect(screen.getByRole("button", { name: "Shuffle: off" })).toBeEnabled());
    } finally {
      release();
      view.unmount();
      timeout.mockRestore();
    }
  });

  it("fails closed for unready output and for read errors, with a retry action", async () => {
    const { state, writes } = api({ available: false, unavailableReason: "Local output is still starting", shuffle: null, repeat: null });
    setup(true);
    await screen.findByText("Local output is still starting");
    expect(screen.getByRole("button", { name: "Shuffle: unavailable" })).toBeDisabled();
    server.use(graphql.query("PlaybackModes", () => HttpResponse.json({ errors: [{ message: "offline" }] })));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("Unable to read playback modes. Refresh to try again.");
    expect(writes).not.toHaveBeenCalled();
    state.available = true;
    api();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Shuffle: off" })).toBeEnabled());
  });

  it("immediately disables local controls when the cast target changes", async () => {
    const { writes } = api({ shuffle: true, repeat: "ONE" });
    const { store } = setup(true);
    await screen.findByRole("button", { name: "Repeat: one" });
    act(() => store.set(currentCastDeviceAtom, { id: "speaker", name: "Speaker" } as Device));
    await screen.findByText("Playback modes are unavailable on this remote receiver.");
    const button = screen.getByRole("button", { name: "Repeat: unavailable" });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(writes).not.toHaveBeenCalled();
  });
});
