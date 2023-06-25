#!/bin/bash
# This script is called by reinitkbd when a keyboard is detected.
# Arguments: $1 = keyboard name, $2 = device node, $3 = xinput device ID, $4 = vendorID:product ID

# Set the keyboard layout. Typically we pass caps:swapescape, but not if it's an
# ultimate hacking keyboard (PID 1d50 VID 6122), which has has the keys swapped in hardware.
echo "Initializing keyboard $1 ($2) with device ID $3 and vendorID:productID $4"
xkbmap_args="-device $3 -layout se,us -option grp:shifts_toggle"

if [ "$4" != "1d50:6122" ]; then
    xkbmap_args="$xkbmap_args -option caps:swapescape"
fi

setxkbmap $xkbmap_args
xkbmap_status=$?

# Set the keyboard repeat rate
xset r rate 200 30
xset_status=$?

# If either command failed, exit with an error code
if [ $xkbmap_status -ne 0 ] || [ $xset_status -ne 0 ]; then
    exit 1
fi
