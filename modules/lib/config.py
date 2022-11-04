# Do not change this class
class Module:
    def __init__(self, filename, types, trigger=None, queried=False):
        self.filename = filename
        self.types = types
        self.trigger = trigger
        self.queried = queried


# You can change the following values
modules = [
    Module("path.sh", "c"),
    Module("files.sh", "fd"),
    Module("hidden.sh", "fd", ".")
] 

default_actions = {
    ';f': "xdg-open {}",
    ';d': "xdg-open {}",
    ';c': "{}"  # just run the command
}

# Only change the separator if this doesn't work for some reason.
# If you change it, you'll need to delete $XDG_DATA_HOME/fzlaunch/history
# (usually ~/.local/share/fzlaunch/history)
sep = " @@ "
num_history = 1000

