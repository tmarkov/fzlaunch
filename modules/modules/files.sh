#!/usr/bin/env bash
find ~ -type f,d \( -name ".*" -o -name "*$SEP*" \) -prune -o \( -printf "less$SEP%p$SEP;%y %p\n" \)

