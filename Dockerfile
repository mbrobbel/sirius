FROM ghcr.io/prefix-dev/pixi:0.59.0 AS base

COPY . /app
WORKDIR /app

RUN pixi install
RUN pixi shell-hook > /shell-hook.sh
RUN echo 'exec "$@"' >> /shell-hook.sh

FROM nvidia/cuda:13.0.2-base-ubuntu24.04 AS build

WORKDIR /app

COPY --from=base /app/.pixi/envs/default /app/.pixi/envs/default
COPY --from=base /shell-hook.sh /shell-hook.sh

ENTRYPOINT ["/bin/bash", "/shell-hook.sh"]
