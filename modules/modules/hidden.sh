#!/usr/bin/env bash
find ~ -type f,d -name "*$SEP*" -prune -o \( -path "*/.*" -printf "less$SEP%p$SEP;%y %p\n" \)
