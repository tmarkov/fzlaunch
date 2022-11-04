#!/usr/bin/env bash
find ${PATH//:/ } -maxdepth 1 -executable -printf "man$SEP%f$SEP;c %f\n" 2> /dev/null
