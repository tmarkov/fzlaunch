#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::input::Candidate;
    use crate::model::Value;

    fn temp_source_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fzlaunch-{name}-{unique}"));
        fs::create_dir(&path).expect("temp source dir should be created");
        path
    }

    fn write_file(path: PathBuf, mode: u32) {
        fs::write(&path, b"#!/bin/sh\n").expect("test executable should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .expect("test executable permissions should be set");
    }

    #[test]
    fn path_source_returns_executables_as_raw_command_candidates() {
        let bin = temp_source_dir("path-source-executable");
        write_file(bin.join("fzlaunch-test-command"), 0o755);

        let candidates = super::executables_from_path(bin.to_str().expect("path should be utf-8"));

        assert_eq!(
            candidates,
            vec![Candidate::new(
                Value::raw("fzlaunch-test-command"),
                'c',
                Some(Value::raw("{}"))
            )]
        );
    }

    #[test]
    fn path_source_ignores_non_executable_files() {
        let bin = temp_source_dir("path-source-non-executable");
        write_file(bin.join("not-executable"), 0o644);

        let candidates = super::executables_from_path(bin.to_str().expect("path should be utf-8"));

        assert_eq!(candidates, Vec::<Candidate>::new());
    }
}
