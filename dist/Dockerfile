# Runtime assembly only: packages/<arch> contains the already tested CLI DEB.
FROM ubuntu:24.04
ARG TARGETARCH
COPY packages/${TARGETARCH}/*.deb /tmp/music-player.deb
RUN apt-get update \
    && apt-get install -y --no-install-recommends /tmp/music-player.deb \
    && rm -rf /var/lib/apt/lists/* /tmp/music-player.deb \
    && groupadd --gid 10001 music-player \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data music-player \
    && install -d -o 10001 -g 10001 /data /data/config /data/cache /music
ENV XDG_CONFIG_HOME=/data/config \
    XDG_CACHE_HOME=/data/cache \
    MUSIC_PLAYER_APPLICATION_DIRECTORY=/data/config/music-player \
    MUSIC_PLAYER_MUSIC_DIRECTORY=/music \
    MUSIC_PLAYER_MODE=server
USER 10001:10001
WORKDIR /data
EXPOSE 5051 5052 5053
ENTRYPOINT ["/usr/bin/music-player"]
