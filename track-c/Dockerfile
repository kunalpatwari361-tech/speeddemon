FROM gcc:15@sha256:eb5e64286ad47187b32bda75c6b6916166ecb41da56559b8b09e0001748d2cce AS native
WORKDIR /src
COPY native/ /src/native/
RUN g++ -std=c++17 -O3 -march=native -fno-exceptions -fno-rtti -c native/parse.cpp -o native/parse.o && ar rcs native/libparse.a native/parse.o

FROM rust:1-slim-trixie@sha256:1a939748a7cbab172de91a014e3804a97af1394a8c844faa67fce9401c4e40fb AS build
WORKDIR /src
COPY . .
COPY --from=native /src/native/libparse.a /src/native/libparse.a
RUN cargo build --release --offline

FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /src/target/release/join /app/join
ENTRYPOINT ["/app/join"]
