#!/usr/bin/env bash
find -L ${PATH//:/ } -maxdepth 1 -executable -not -name ".*" -printf "man$SEP%f$SEP;c %f\n" 2> /dev/null
