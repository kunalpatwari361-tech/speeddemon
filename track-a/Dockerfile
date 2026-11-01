FROM python:3.13-slim@sha256:d98cc29d3266e535dd9da1099688d9cb3ca4753813571e071763d17725460962 AS profile-data
WORKDIR /profile
COPY generator.py .
RUN python3 generator.py --seed 2 --scale full --out /profile/data

FROM gcc:15@sha256:eb5e64286ad47187b32bda75c6b6916166ecb41da56559b8b09e0001748d2cce AS native
WORKDIR /src
COPY native/ /src/native/
RUN g++ -std=c++17 -O3 -march=native -fno-exceptions -fno-rtti -c native/scan.cpp -o native/scan.o && ar rcs native/libscan.a native/scan.o

FROM halloumee/speed-demon-rust:1.99.1-trixie-llvm-tools@sha256:13e4ee7135d8eac6c56476c7d1aee201f36479f4745fd5d0e6b4529f4e13251a AS build
WORKDIR /src
COPY . .
COPY --from=native /src/native/libscan.a /src/native/libscan.a
RUN mkdir -p /tmp/pgo && RUSTFLAGS="-C target-cpu=native -L /src/native -Cprofile-generate=/tmp/pgo" cargo build --release --offline
COPY --from=profile-data /profile/data /data
RUN mkdir -p /out && TRACK=a /src/target/release/ingest
RUN llvm-profdata merge --output=/tmp/pgo/merged.profdata /tmp/pgo/*.profraw
RUN RUSTFLAGS="-C target-cpu=native -L /src/native -Cprofile-use=/tmp/pgo/merged.profdata" cargo build --release --offline

FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /src/target/release/ingest /app/ingest
ENTRYPOINT ["/app/ingest"]
