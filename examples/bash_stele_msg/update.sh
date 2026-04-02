#!/bin/sh

# Update the global config.
update_config() {
    stele msg config --background '#181818'
}

# Update the time module.
update_time() {
    stele msg module --id time_module --alignment center \
        --layer '{ "content": "#282828" }' \
        --layer '{ "content": "'$(date +%H:%M:%S)'", "margin": { "left": 25, "right": 25 } }'
}

# Initialize bar.
update_time
update_config

# Update time every second.
while true; do
    sleep 1
    update_time
done
