### Manual installed-host proof

**Setup: Fresh standalone install (checkbox off)**
```
$ GAH_FACTORY_ENABLED=false bash scripts/bootstrap.sh standalone
...
Setup complete.
```
```
$ systemctl --user status gah-loop@test-repo.service
Unit gah-loop@test-repo.service could not be found.

$ systemctl --user status gah-server.service
● gah-server.service - GAH Server
     Loaded: loaded (/home/user/.config/systemd/user/gah-server.service; enabled; preset: enabled)
     Active: active (running)
```
Dashboard setup check shows "Factory module disabled · local application remains available."

**Setup: Enable and explicitly start a configured loop**
```
$ gah config set --factory-enabled true
$ systemctl --user start gah-loop@test-repo.service
```
```
$ systemctl --user status gah-loop@test-repo.service
● gah-loop@test-repo.service - GAH Dispatch Loop (test-repo)
     Loaded: loaded (/home/user/.config/systemd/user/gah-loop@.service; disabled; preset: enabled)
     Active: active (running)
```

**Setup: Disable and verify stopped/disabled state**
```
$ gah config set --factory-enabled false
```
```
$ systemctl --user status gah-loop@test-repo.service
○ gah-loop@test-repo.service - GAH Dispatch Loop (test-repo)
     Loaded: loaded (/home/user/.config/systemd/user/gah-loop@.service; disabled; preset: enabled)
     Active: inactive (dead)
```
The shared services (`gah-server.service`, `gah-worker.service`, `gah-prune.timer`) remain active and unaffected.

**Setup: Upgrade with legacy config (no defaults.factory_enabled)**
```
$ cat ~/.config/gah/gah-config.toml
[defaults]
current_manager = "claude"
```
```
$ gah config show --json --full | grep factory_enabled
  "factory_enabled": true,
```
Dashboard setup check shows "Factory module enabled".

**Setup: Upgrade with explicit off config**
```
$ cat ~/.config/gah/gah-config.toml
[defaults]
factory_enabled = false
```
```
$ gah config show --json --full | grep factory_enabled
  "factory_enabled": false,
```
Dashboard setup check shows "Factory module disabled".
