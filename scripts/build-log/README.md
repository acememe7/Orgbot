# Build log

The public page at https://orgbots.theaecestudio.com/build/ and the local
`build.html` at the repository root.

- `builds.json`: every build, oldest first. Add an entry for each new build.
- `render.py`: writes `website/build/index.html` and `build.html` from it, and
  prints the JSON-LD hash the site's Content-Security-Policy must list
  (`deploy/website/nginx-orgbots.*.conf`).
- `scene.mjs`: the 3D room. Bundle it with three.js (0.185.1) into
  `website/build/scene.js`:

      esbuild scripts/build-log/scene.mjs --bundle --minify --format=iife \
        --target=es2020 --legal-comments=eof --alias:three=<three package dir> \
        --outfile=website/build/scene.js
