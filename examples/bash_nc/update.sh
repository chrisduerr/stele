#!/bin/sh

# Send a message to all Stele IPC sockets.
send_message() {
    for socket in /var/run/user/1000/stele-*.sock; do
        printf "$1" | nc -U $socket
    done
}

# Update the global config.
update_config() {
    send_message '{ "type": "config", "background": ["#181818"] }'
}

# Update the time module.
update_time() {
    send_message '{ "type": "module", "id": "time_module", "alignment": "center", "layers": [{
        "content": "#282828"
    }, {
        "content": "'$(date +%H:%M:%S)'",
        "margin": { "left": 25, "right": 25 }
    }] }'
}

# Initialize bar.
update_time
update_config

# Update time every second.
while true; do
    sleep 1
    update_time
done
