#!/bin/bash

proxy_check=$(redis-cli ping)
proxy_check_rc=$?

if [ "$proxy_check_rc" = "0" ]
then
    # TCP connection was established, check for pong response
    if [ "$(echo $proxy_check | grep -c PONG)" = "0" ]
    then
        # Pong was NOT found, exit 1
        echo "pong not found in proxy response, got $proxy_check"
        exit 1
    fi
else
    # Connection could not be established, exit 1
    echo "Could not establish tcp connection to 0.0.0.0:6379"
    exit 1
fi

# Clean exit
exit
