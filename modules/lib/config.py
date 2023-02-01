# Do not change this class
class Module:
    def __init__(self, filename, types, trigger=None, takes_query=False):
        self.filename = filename
        self.types = types
        self.trigger = trigger
        self.takes_query = takes_query


# You can change the following values
modules = [
    Module("path.sh", "c"),
    Module("files.sh", "fd"),
    Module("hidden.sh", "fd", "."),
    Module("calculator.py", "t", "=", True),
] 

default_actions = {
    ';f': "xdg-open {}", # File: open
    ';d': "xdg-open {}", # Directory: open
    ';c': "{}",  # Command: run
    ';t': "echo '{}' | wl-copy", # Text: copy to clipboard
}

# Only change the separator if this doesn't work for some reason.
# If you change it, you'll need to delete $XDG_DATA_HOME/fzlaunch/history
# (usually ~/.local/share/fzlaunch/history)
sep = " @@ "
num_history = 1000

