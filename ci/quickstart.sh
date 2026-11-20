#!/usr/bin/env bash
# Runs the README's quick start as written, with the controller simulator in
# place of a USB stick: download the release and check its sum, then write
# the README's configuration, run the bridge with the README's command,
# start inclusion with the README's command, press the simulated switch's
# button, and wait for its Home Assistant discovery message and its value.
# It runs twice: with the release binary the README downloads, and with this
# commit's build.
#
# Needs Linux, sudo, cargo, curl, jq, and mosquitto with its clients.
set -euo pipefail
cd "$(dirname "$0")/.."

# The body of the code block that follows `<!-- quickstart:NAME -->`.
block() {
    awk -v marker="<!-- quickstart:$1 -->" '
        $0 == marker { found = 1; next }
        found && /^```/ { if (inside) exit; inside = 1; next }
        inside { print }
    ' README.md
}

SIM_PORT=4545
work="$(mktemp -d)"
pids=()
cleanup() {
    exec 3>&- 2>/dev/null || true
    for pid in "${pids[@]}"; do sudo kill "${pid}" 2>/dev/null || true; done
    rm -rf "${work}"
}
trap cleanup EXIT

# Wait up to $3 seconds for a message on topic $1 that the jq filter $2
# accepts. Retained messages count: the bridge may publish before we listen.
wait_for() {
    local deadline=$((SECONDS + $3))
    while [ "${SECONDS}" -lt "${deadline}" ]; do
        if mosquitto_sub -C 1 -W 5 -t "$1" 2>/dev/null | jq -e -R "(. as \$raw | try fromjson catch \$raw) | $2" >/dev/null 2>&1; then
            return 0
        fi
    done
    echo "timed out waiting for $1 ($2)" >&2
    cat "${work}/bridge.log" "${work}/sim.log" >&2
    return 1
}

echo "--- 1. Download, verify and install the release"
download="$(block download)"
(cd "${work}" && bash -euxo pipefail -c "${download}")

echo "--- Build the simulator and this commit's bridge"
cargo build --locked -p zwave-bridge -p zwave-sim
build="$(pwd)/target/debug/zwave-bridge"

# A fresh broker for each run, so no retained message from an earlier run
# can satisfy a check. The README's commands use the default port.
sudo systemctl stop mosquitto 2>/dev/null || true

for bridge in zwave-bridge "${build}"; do
    echo "=== quick start with ${bridge}"
    sudo rm -rf /var/lib/zwave-bridge
    mosquitto -p 1883 >"${work}/broker.log" 2>&1 &
    pids+=("$!")
    # The simulator takes commands on standard input: keep a FIFO open to it.
    rm -f "${work}/sim.in"
    mkfifo "${work}/sim.in"
    target/debug/zwave-sim "127.0.0.1:${SIM_PORT}" <"${work}/sim.in" >"${work}/sim.log" 2>&1 &
    pids+=("$!")
    exec 3>"${work}/sim.in"
    sleep 1

    echo "--- 3. The README's configuration, with the simulator as the port"
    sudo mkdir -p /etc/zwave-bridge
    block config \
        | sed "s|^port = .*|port = \"tcp://127.0.0.1:${SIM_PORT}\"|" \
        | sudo tee /etc/zwave-bridge/config.toml

    echo "--- 4. Run"
    run="$(block run | sed "s|^sudo zwave-bridge |sudo ${bridge} |")"
    bash -c "${run}" >"${work}/bridge.log" 2>&1 &
    wait_for zwave/bridge/state '. == "online"' 30
    home="$(mosquitto_sub -C 1 -W 10 -t zwave/bridge/info | jq -r .home_id)"

    echo "--- 5. Include, and press the switch's button"
    bash -euxo pipefail -c "$(block pair)"
    sleep 2
    echo "press switch" >&3
    wait_for "homeassistant/switch/zwave_${home}_2/binary_switch_current/config" \
        '.state_topic == "zwave/nodes/2/binary_switch/current"' 90
    wait_for zwave/nodes/2/binary_switch/current '.value | type == "boolean"' 60

    sudo pkill -f -- "zwave-bridge --config" || true
    sleep 1
    exec 3>&-
    for pid in "${pids[@]}"; do kill "${pid}" 2>/dev/null || true; done
    pids=()
    sleep 1
done
echo "quick start: ok"
