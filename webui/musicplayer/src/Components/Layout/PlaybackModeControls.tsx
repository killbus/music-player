import { usePlaybackModes, type PlaybackModeControlsState } from "../../Hooks/usePlaybackModes";
import { IconButton, Icons } from "../UI";

type Props = { modes?: PlaybackModeControlsState; className?: string };
const repeatNames = { OFF: "off", ALL: "all", ONE: "one" } as const;
const repeatActions = { OFF: "Repeat all", ALL: "Repeat one", ONE: "Turn repeat off" } as const;

export function ShuffleControl({ modes, className }: Props) {
  const label = `Shuffle: ${modes?.shuffle == null ? "unavailable" : modes.shuffle ? "on" : "off"}`;
  return <IconButton
    icon={Icons.shuffle} iconSize={15} className={className}
    aria-label={label} aria-pressed={!!modes?.shuffle}
    accented={!!modes?.shuffle} disabled={!modes || modes.disabled}
    title={modes?.message ?? `${modes?.shuffle ? "Turn shuffle off" : "Shuffle upcoming tracks"}. Turning off does not restore the previous order.`}
    onClick={modes?.toggleShuffle}
  />;
}

export function RepeatControl({ modes, className }: Props) {
  const repeat = modes?.repeat;
  return <span className={className}>
    <span className="relative inline-flex">
      <IconButton
        icon={Icons.repeat} iconSize={15}
        aria-label={`Repeat: ${repeat ? repeatNames[repeat] : "unavailable"}`}
        aria-pressed={!!repeat && repeat !== "OFF"}
        accented={!!repeat && repeat !== "OFF"} disabled={!modes || modes.disabled}
        title={modes?.message ?? (repeat ? repeatActions[repeat] : "Repeat unavailable")}
        onClick={modes?.cycleRepeat}
      />
      {repeat === "ONE" && <span aria-hidden="true" className="pointer-events-none absolute right-0 top-0 text-[10px] font-bold text-accent">1</span>}
    </span>
  </span>;
}

export function ModeNotice({ modes }: Props) {
  if (!modes?.message && !modes?.error) return null;
  return <div className="flex items-center gap-2 text-xs text-fg">
    <span role={modes.error ? "alert" : "status"}>{modes.error ?? modes.message}</span>
    {!modes.pending && <button type="button" onClick={modes.refresh} className="rounded-control px-2 py-1 text-accent underline">Refresh</button>}
  </div>;
}

/** The queue sheet is also the mode-control surface on narrow screens. */
export function QueuePlaybackModes() {
  const modes = usePlaybackModes();
  return <section aria-label="Playback modes" className="space-y-2 border-t border-line px-4 py-2">
    <div className="flex items-center gap-2">
      <ShuffleControl modes={modes} />
      <span className="flex-1 text-xs text-dim">Shuffle {modes.shuffle == null ? "unavailable" : modes.shuffle ? "on" : "off"}</span>
      <RepeatControl modes={modes} />
      <span className="text-xs text-dim">Repeat {modes.repeat ? repeatNames[modes.repeat] : "unavailable"}</span>
    </div>
    <ModeNotice modes={modes} />
    <p className="text-[11px] text-dim">Shuffle changes upcoming tracks. Turning it off keeps their current order.</p>
  </section>;
}
