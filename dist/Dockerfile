# Runtime assembly only: binaries/<arch> contains the canonical CLI executable.
FROM debian:trixie-slim
ARG TARGETARCH
# Keep this list aligned with CI ldd/runtime evidence for both architectures.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libasound2t64 libstdc++6 libgcc-s1 libc6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 music-player \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data music-player \
    && install -d -o 10001 -g 10001 /data /data/config /data/cache /music
COPY --chmod=0755 binaries/${TARGETARCH}/music-player /usr/bin/music-player
COPY LICENSE /usr/share/licenses/music-player/LICENSE
ENV XDG_CONFIG_HOME=/data/config \
    XDG_CACHE_HOME=/data/cache \
    MUSIC_PLAYER_APPLICATION_DIRECTORY=/data/config/music-player \
    MUSIC_PLAYER_MUSIC_DIRECTORY=/music \
    MUSIC_PLAYER_MODE=server
USER 10001:10001
WORKDIR /data
EXPOSE 5051 5052 5053
ENTRYPOINT ["/usr/bin/music-player"]
