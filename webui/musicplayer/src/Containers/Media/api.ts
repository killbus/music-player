import { useInfiniteQuery } from "@tanstack/react-query";
import { getApiUrl } from "../../Api/fetcher";
import type { Track } from "../../Hooks/GraphQL";

// Only the metadata AddTracks accepts. Handles go through unchanged; neither
// this query nor the page asks for a stream URL or a cover route.
export type MediaTrack = Pick<
  Track,
  "id" | "title" | "uri" | "duration" | "discNumber" | "trackNumber"
>;

export type MediaEntry = {
  id: string;
  title: string;
  itemType: string | null;
  mediaType: string | null;
  isContainer: boolean;
  seasonNumber: string | null;
  episodeNumber: string | null;
  track: MediaTrack | null;
};

export type MediaPage = {
  serverId: string;
  nextOffset: number | null;
  entries: MediaEntry[];
};

export type MediaBrowserResult = {
  mediaBrowser: { serverId: string; supported: boolean } | null;
};

const TRACK_FIELDS = "id title uri duration discNumber trackNumber";
const BROWSER = `query MediaBrowser { mediaBrowser { serverId supported } }`;
const BROWSE = `
  query BrowseMedia($serverId: ID!, $parent: ID, $offset: Int!, $limit: Int!) {
    browseMedia(serverId: $serverId, parent: $parent, offset: $offset, limit: $limit) {
      serverId nextOffset
      entries {
        id title itemType mediaType isContainer seasonNumber episodeNumber
        track { ${TRACK_FIELDS} }
      }
    }
  }
`;
const CONTAINER_TRACKS = `
  query MediaContainerTracks($serverId: ID!, $parent: ID!, $offset: Int!, $limit: Int!) {
    mediaContainerTracks(serverId: $serverId, parent: $parent, offset: $offset, limit: $limit) {
      ${TRACK_FIELDS}
    }
  }
`;

// The generated fetcher has no AbortSignal parameter. Keep these few documents
// here until codegen includes the media API, using the same endpoint/credentials.
async function request<T>(
  query: string,
  variables: Record<string, unknown>,
  signal: AbortSignal
): Promise<T> {
  signal.throwIfAborted();
  const response = await fetch(getApiUrl(), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ query, variables }),
    signal,
  });
  if (!response.ok) throw new Error(`Media request failed (${response.status})`);
  const result: { data?: T; errors?: { message: string }[] } = await response.json();
  signal.throwIfAborted();
  // Reject partial GraphQL data too: a partial container must never be queued.
  if (result.errors?.length) throw new Error(result.errors[0].message);
  if (!result.data) throw new Error("The server returned no media data");
  return result.data;
}

export const browserKey = (serverId: string | null) => ["MediaBrowser", serverId] as const;
export const fetchMediaBrowser = (signal: AbortSignal) =>
  request<MediaBrowserResult>(BROWSER, {}, signal);

export function useMediaPages(serverId: string, parent: string | null) {
  return useInfiniteQuery({
    queryKey: ["BrowseMedia", serverId, parent],
    initialPageParam: 0,
    queryFn: async ({ pageParam, signal }) => {
      const data = await request<{ browseMedia: MediaPage }>(
        BROWSE,
        { serverId, parent, offset: pageParam, limit: 100 },
        signal
      );
      const page = data.browseMedia;
      if (!page || page.serverId !== serverId) {
        throw new Error("The browsing server changed; refresh its libraries");
      }
      if (page.nextOffset !== null &&
          (!Number.isInteger(page.nextOffset) || page.nextOffset <= pageParam)) {
        throw new Error("The server returned an invalid next page");
      }
      return page;
    },
    getNextPageParam: (lastPage) => lastPage.nextOffset ?? undefined,
    // Never show the previous parent's or account's rows during a new request.
    placeholderData: undefined,
    retry: false,
  });
}

export async function fetchContainerTracks(
  serverId: string, parent: string, signal: AbortSignal
): Promise<MediaTrack[]> {
  const data = await request<{ mediaContainerTracks: MediaTrack[] }>(
    CONTAINER_TRACKS, { serverId, parent, offset: 0, limit: 0 }, signal
  );
  if (!Array.isArray(data.mediaContainerTracks)) {
    throw new Error("The server returned no container tracks");
  }
  return data.mediaContainerTracks;
}

export const isSourceMutation = (key?: readonly unknown[]) =>
  ["ConnectToServer", "DisconnectFromServer", "DeleteServer"].includes(String(key?.[0]));
