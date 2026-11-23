import { useQuery } from "@tanstack/react-query";
import { useAtomValue } from "jotai";
import { fetcher } from "../Api/fetcher";
import { currentCastDeviceAtom } from "../State";

type SourcePlayback = {
  source: string;
  occurrenceId: string;
  phase: "STOPPED" | "PAUSED" | "RESOLVING" | "OPENING" | "RECOVERING" | "STREAMING" | "DRAINING" | "FAILED" | "END_UNCONFIRMED" | "ENDED";
  error: string | null;
  retryAttempt: number;
  retryLimit: number;
};

export function useSourcePlayback(source?: string) {
  const receiver = useAtomValue(currentCastDeviceAtom);
  const enabled = !!source?.startsWith("mp-source:") && !receiver;
  const query = useQuery({
    queryKey: ["SourcePlayback", source, receiver?.id ?? "local"],
    queryFn: ({ signal }) => fetcher<{ sourcePlayback: SourcePlayback | null }, Record<string, never>>(
      `query SourcePlayback { sourcePlayback { source occurrenceId phase error retryAttempt retryLimit } }`,
      {}, undefined, AbortSignal.any([signal, AbortSignal.timeout(5000)])
    )(),
    enabled,
    refetchInterval: 1000,
    retry: false,
  });
  const observed = query.data?.sourcePlayback;
  const status = enabled && observed?.source === source ? observed : null;
  const starting = status?.phase === "RESOLVING" || status?.phase === "OPENING" || status?.phase === "RECOVERING";
  let error = status?.error ?? null;
  let message: string | null = null;
  switch (status?.phase) {
    case "RECOVERING":
      error = null;
      message = `Recovering playback… (${status.retryAttempt}/${status.retryLimit})`;
      break;
    case "RESOLVING": message = "Resolving audio…"; break;
    case "OPENING": message = "Starting audio…"; break;
    case "FAILED": error ??= "Playback failed. Press Play to retry."; break;
    case "END_UNCONFIRMED":
      error ??= "Audio ended before completion could be confirmed. Press Play to resume.";
      break;
  }
  if (enabled && query.isError) error = query.error.message;
  return { starting, error, message };
}
