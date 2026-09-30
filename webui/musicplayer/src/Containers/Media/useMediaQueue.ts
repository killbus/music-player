import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useLayoutEffect, useRef, useState } from "react";
import {
  useAddTracksMutation,
  type GetConnectedServerQuery,
} from "../../Hooks/GraphQL";
import { usePlayback } from "../../Hooks/usePlayback";
import { usePlayTrack } from "../../Hooks/usePlayTrack";
import {
  browserKey, fetchContainerTracks, isSourceMutation, type MediaBrowserResult,
} from "./api";

export const errorMessage = (error: unknown) =>
  error instanceof Error ? error.message : "The media request failed";

export function useMediaQueue(serverId: string) {
  const client = useQueryClient();
  const addTracks = useAddTracksMutation();
  const playTrack = usePlayTrack();
  const { playNext } = usePlayback();
  const operation = useRef<AbortController | null>(null);
  const sent = useRef(false);
  const [phase, setPhase] = useState<"idle" | "reading" | "submitting">("idle");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  const invalidate = useCallback(() => {
    operation.current?.abort();
    operation.current = null;
  }, []);

  const sourceIsCurrent = useCallback(() => {
    const connected = client.getQueryState<GetConnectedServerQuery>(["GetConnectedServer"]);
    const browser = client.getQueryState<MediaBrowserResult>(browserKey(serverId));
    return connected?.data?.connectedServer?.id === serverId &&
      !connected.isInvalidated && !browser?.isInvalidated &&
      browser?.data?.mediaBrowser?.serverId === serverId &&
      browser.data.mediaBrowser.supported &&
      client.isMutating({ predicate: (m) => isSourceMutation(m.options.mutationKey) }) === 0;
  }, [client, serverId]);

  const cancel = useCallback(() => {
    invalidate();
    setPhase("idle");
    setMessage(sent.current
      ? "The queue request was already sent; it has not been undone."
      : "Queue request cancelled.");
    setError("");
  }, [invalidate]);

  useLayoutEffect(() => {
    // Cache notifications are synchronous. Revoke the pending read at switch
    // intent / invalidation, before React has rendered the new account. A→B→A
    // must not make an old A request current again.
    const stopIfStale = () => {
      if (operation.current && !sourceIsCurrent()) cancel();
    };
    const unsubscribeQuery = client.getQueryCache().subscribe(stopIfStale);
    const unsubscribeMutation = client.getMutationCache().subscribe(stopIfStale);
    return () => {
      unsubscribeQuery();
      unsubscribeMutation();
      invalidate();
    };
  }, [client, cancel, invalidate, sourceIsCurrent]);

  const begin = (nextPhase: "reading" | "submitting") => {
    invalidate();
    const controller = new AbortController();
    operation.current = controller;
    sent.current = false;
    setPhase(nextPhase);
    setMessage("");
    setError("");
    return controller;
  };
  const owns = (controller: AbortController) =>
    operation.current === controller && !controller.signal.aborted;

  const queueContainer = async (parent: string) => {
    const controller = begin("reading");
    try {
      if (!sourceIsCurrent()) throw new Error("The browsing server changed; refresh its libraries");
      const tracks = await fetchContainerTracks(serverId, parent, controller.signal);
      // This is the commit boundary. No await between checking ownership and
      // sending the single AddTracks mutation. Nothing is added on read failure.
      if (!owns(controller) || !sourceIsCurrent()) return;
      if (!tracks.length) {
        setMessage("No audio tracks in this container.");
        return;
      }
      setPhase("submitting");
      sent.current = true;
      const result = await addTracks.mutateAsync({
        tracks: tracks.map(({ id, title, uri, duration, discNumber, trackNumber }) =>
          ({ id, title, uri, duration, discNumber, trackNumber })),
      });
      if (!result.addTracks) throw new Error("The queue update was not accepted");
      void client.invalidateQueries({ queryKey: ["GetTracklist"] });
      if (owns(controller)) setMessage(`Added ${tracks.length} tracks to the queue.`);
    } catch (cause) {
      if (owns(controller)) setError(errorMessage(cause));
    } finally {
      if (owns(controller)) {
        operation.current = null;
        setPhase("idle");
      }
    }
  };

  const leaf = async (id: string, next: boolean) => {
    const controller = begin("submitting");
    try {
      if (!sourceIsCurrent()) throw new Error("The browsing server changed; refresh its libraries");
      sent.current = true;
      if (next) await playNext({ trackId: id });
      else await playTrack(id);
      if (owns(controller)) setMessage(next ? "Queued to play next." : "Audio playback requested.");
    } catch (cause) {
      if (owns(controller)) setError(errorMessage(cause));
    } finally {
      if (owns(controller)) {
        operation.current = null;
        setPhase("idle");
      }
    }
  };

  return { phase, message, error, cancel, queueContainer, leaf };
}
