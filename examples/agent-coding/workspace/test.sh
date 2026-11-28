#!/bin/sh
set -eu

check() {
    expected=$1
    shift
    actual=$(/bin/sh ./greet.sh "$@")
    if [ "$actual" != "$expected" ]; then
        printf 'expected "%s", got "%s"\n' "$expected" "$actual" >&2
        exit 1
    fi
}

check 'Hello, world!'
check 'Hello, Ada!' Ada
printf '2 checks passed\n'
