# The image `cargo xtask capture` starts an emulator in: Flatpak, a virtual display and the buses
# Flatpak's sandbox needs. Run it --privileged (bubblewrap cannot make its namespaces otherwise),
# with a volume for the Flatpak installation so runtimes are fetched once:
#
#   docker build -t hermir-capture ci/capture
#   docker run --rm --privileged -v hermir-flatpak:/fp -v "$PWD/out:/out" \
#       hermir-capture net.pcsx2.PCSX2 25 /out
FROM ubuntu:24.04
RUN apt-get update -q \
 && apt-get install -y -q --no-install-recommends ca-certificates flatpak xvfb xauth dbus dbus-x11 libx11-6 python3 \
 && rm -rf /var/lib/apt/lists/*
ENV FLATPAK_USER_DIR=/fp
COPY capture.sh close-windows.py /opt/hermir/
ENTRYPOINT ["/opt/hermir/capture.sh"]
