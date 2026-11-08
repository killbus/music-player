import gql from "graphql-tag";
import { TRACK_FRAGMENT } from "../Fragments";

export const GET_CURRENTLY_PLAYING = gql`
  query CurrentlyPlayingSong {
    currentlyPlayingSong {
      track {
        ...TrackFragment
        artists {
          id
          name
        }
        album {
          id
          title
          cover
        }
      }
      index
      isPlaying
      positionMs
    }
  }
  ${TRACK_FRAGMENT}
`;

export const GET_PLAYER_RUNTIME = gql`
  query PlayerRuntime {
    playerRuntime {
      output
      phase
      error
    }
  }
`;
