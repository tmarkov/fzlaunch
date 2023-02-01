import sys
import os
import subprocess
from lib import history, config
from xdg import BaseDirectory

FILE = __file__

CACHE_DIR = BaseDirectory.save_cache_path("fzlaunch")
DATA_DIR = BaseDirectory.save_data_path("fzlaunch")
PROJECT_DIR = os.path.abspath(os.path.dirname(__file__))
MODULE_DIRS = [
    os.path.join(DATA_DIR, "modules"),
    os.path.join(PROJECT_DIR, "modules")
]

EOF_SIGNAL = "%%>>DONE<<%%"

def valid_type(allowed_types, module_types):
    if allowed_types == "":
        return True

    for c in module_types:
        if c in allowed_types:
            return True

    return False


def get_path(module):
    for dir in MODULE_DIRS:
        path = os.path.join(dir, module.filename)
        if os.path.exists(path):
            return path

    raise ValueError("Module not found")


if __name__ == "__main__":
    sys.pycache_prefix = CACHE_DIR
    if len(sys.argv) >= 2:
        types = sys.argv[1];
    else:
        types = ""

    hist_trim = history.print_history()
    sys.stdout.flush()

    env = {
        **os.environ,
        'SEP': config.sep
    }

    popens = []

    if hist_trim:
        popens.append(hist_trim)

    for module in config.modules:
        if module.trigger is None and valid_type(types, module.types):
            popens.append(subprocess.Popen(get_path(module), env=env))

    for l in sys.stdin:
        if l.strip() == EOF_SIGNAL:
            break

        kw = l.split()
        if len(kw) == 0:
            continue

        if not (l[-1] == ' ' or (l[-1] == '\n' and l[-2] == ' ')):
            kw = kw[:-1]

        for module in config.modules:
            if not valid_type(types, module.types) or module.trigger is None:
                continue
            if not module.takes_query and module.trigger in kw:
                popens.append(subprocess.Popen(get_path(module), env=env))
                module.trigger = None
            if module.takes_query and len(kw) > 0 and kw[-1] == module.trigger:
                query = l.strip()
                popens.append(subprocess.Popen([get_path(module), query], env=env));

    for popen in popens:
        popen.wait()

