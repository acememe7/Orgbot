# Install OrgBots on Ubuntu

OrgBots is distributed through a signed third-party APT repository. The desktop
package targets Ubuntu 24.04 amd64. Adding the repository key and source is a
one-time setup on each computer. The archive key fingerprint is
`81445722C3B9265312C12039F831B11611E6D198`.

The installer needs `curl`, `gpg`, `ca-certificates`, and standard core utilities.
Install any missing prerequisites first:

```sh
sudo apt update
sudo apt install curl gnupg ca-certificates coreutils
```

Review the [installer source](../scripts/install-orgbots.sh), download it, and
run it with Bash:

```sh
curl -fsSLo install-orgbots.sh \
  https://raw.githubusercontent.com/acememe7/Orgbot/main/scripts/install-orgbots.sh
sed -n '1,220p' install-orgbots.sh
bash install-orgbots.sh
```

The script checks the key's full fingerprint before changing APT files. It
creates a Deb822 source scoped with `Signed-By`, refreshes package metadata, and
asks APT to install `orgbots`. Keep APT's normal confirmation prompt and review
the package list before accepting. It uses `sudo` only for system changes and
does not disable TLS checks or install missing prerequisites automatically.
If a key or source file at the install path differs, the installer preserves
it beside the file as `.orgbots-backup.*` before replacing it. Matching files
with the expected root ownership and permissions are left untouched.

After setup, install later updates with:

```sh
sudo apt update
sudo apt install orgbots
```

Launch **OrgBots** from your desktop application menu, or run `orgbots` in a
terminal. Configure at least one provider under **Connections & tools** before starting a
bot, using that provider's API credential or supported CLI sign-in.

From 2.0.1, new installations store recovery copies in
`~/.local/share/orgbots/trash` (or `$XDG_DATA_HOME/orgbots/trash`). Choose a
different private folder under **Connections & tools → File recovery**. An existing HDD
selection is preserved, and missing selected storage stops replacement/removal
without falling back to another disk. Older package-only installations require
an explicit folder selection on upgrade. Installing the package does not mount
disks or enable permanent cleanup. See the
[recovery guide](../app/linux/TRASH.md) for the optional seven-day cleanup timer
and [workspace safety guide](WORKSPACE_SAFETY.md) for integration boundaries.

To remove the application package, run `sudo apt remove orgbots`. The APT
source and scoped key remain available for future installs and updates.

See the [APT distribution guide](apt-distribution.md) for package layout,
signing, and archive maintenance details.
