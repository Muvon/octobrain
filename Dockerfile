# Copyright 2026 Muvon Un Limited
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Single-stage Dockerfile for octobrain — downloads pre-built static binary
# from GitHub Releases instead of compiling from source.
# Build with: docker build --build-arg OCTOBRAIN_VERSION=0.8.0 .
FROM debian:bookworm-slim

ARG OCTOBRAIN_VERSION
ARG TARGETARCH

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
		ca-certificates \
		curl \
		&& rm -rf /var/lib/apt/lists/* \
		&& update-ca-certificates

# Map Docker TARGETARCH to the release asset target triple
# amd64 → x86_64-unknown-linux-musl
# arm64 → aarch64-unknown-linux-musl
RUN set -eu; \
		case "${TARGETARCH}" in \
			amd64)  ASSET_TARGET="x86_64-unknown-linux-musl" ;; \
			arm64)  ASSET_TARGET="aarch64-unknown-linux-musl" ;; \
			*) echo "unsupported arch ${TARGETARCH}"; exit 1 ;; \
		esac; \
		ASSET="octobrain-${OCTOBRAIN_VERSION}-${ASSET_TARGET}.tar.gz"; \
		URL="https://github.com/muvon/octobrain/releases/download/${OCTOBRAIN_VERSION}/${ASSET}"; \
		echo "Downloading ${URL}"; \
		curl -fsSL "${URL}" -o /tmp/octobrain.tar.gz; \
		tar xzf /tmp/octobrain.tar.gz -C /tmp; \
		mv /tmp/octobrain /usr/local/bin/octobrain; \
		chmod +x /usr/local/bin/octobrain; \
		rm /tmp/octobrain.tar.gz

# Create a non-root user
RUN groupadd -r octobrain && useradd -r -g octobrain octobrain

# Create data directory
RUN mkdir -p /data && chown -R octobrain:octobrain /data

# Create app directory
WORKDIR /app

# Change ownership to non-root user
RUN chown -R octobrain:octobrain /app

# Switch to non-root user
USER octobrain

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
		CMD octobrain --help || exit 1

# Set the entrypoint
ENTRYPOINT ["octobrain"]
CMD ["--help"]