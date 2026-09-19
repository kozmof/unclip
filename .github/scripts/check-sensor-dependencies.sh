#!/usr/bin/env bash
# Check normal dependencies on every target, including transitive dependencies.
set -eu

dependency_tree=$(cargo tree --locked --color never -p unclip-sensors --edges normal --prefix none --format '{p}' --target all)
printf '%s\n' "$dependency_tree" | awk '
    $1 ~ /^(tokio(-.*)?|async-std|smol|reqwest|hyper(-.*)?|ureq|surf|isahc|rand(_.*)?|getrandom|fastrand|chrono|time|jiff)$/ {
        print "Forbidden sensor dependency: " $1 > "/dev/stderr"
        forbidden = 1
    }
    END { exit forbidden }
'
