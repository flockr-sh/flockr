# AUR packages

- `flockr-bin`: the release binaries from GitHub.
- `flockr-git`: built from `master`.

Each lives in its own AUR git repo. To publish or update:

```bash
git clone ssh://aur@aur.archlinux.org/flockr-bin.git /tmp/aur-flockr-bin
cp packaging/aur/flockr-bin/PKGBUILD /tmp/aur-flockr-bin/
cd /tmp/aur-flockr-bin
updpkgsums                # flockr-bin only, after the GitHub release is out
makepkg -si               # check that it builds and installs
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO && git commit -m "Update to <version>" && git push
```

For a new release, bump `pkgver` in `flockr-bin/PKGBUILD` and reset
`pkgrel=1`. `flockr-git` only needs a push when the PKGBUILD itself changes.
