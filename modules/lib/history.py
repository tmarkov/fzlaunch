#!/usr/bin/env python

import os
import subprocess
from xdg import BaseDirectory

from . import config

DATA_DIR = BaseDirectory.save_data_path("fzlaunch")
HIST_FILE = os.path.join(DATA_DIR, "history")


def new_entry(preview, item, type, name):
    if preview and item and type and name:
        with open(HIST_FILE, "a") as f:
            f.write(f"{preview}{config.sep}{item}{config.sep}{type} {name}\n")


def print_history():
    history = {}

    if not os.path.exists(HIST_FILE):
        return

    n = 0
    with open(HIST_FILE, 'r') as f:
        for line in f.readlines():
            n += 1
            line = line.strip()
            if line in history:
                history[line] += 1
            else:
                history[line] = 1

    for item in sorted(history, key=lambda v: -history[v]):
        if len(item) > 0:
            print(item)

    max_history = int(config.num_history) + 100
    if n > max_history:
        return subprocess.Popen(f"sed -i -e :a -e '$q;N;{max_history + 1},$D;ba' {HIST_FILE}")

    return None

