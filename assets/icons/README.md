# Icons

- `velo.svg`: Velo's own app icon.
- `lucide/`: icons from [Lucide](https://lucide.dev) `lucide-static@0.460.0`, ISC license (`lucide/LICENSE`). The SVGs use `currentColor`, so the UI tints them with `Image.colorize` from the theme.

Never add IDM's icons or artwork here.

`packaging/icons/` holds `velo.svg` rendered to PNG for the installers (`.icns` takes 32, 128, 256, 512 and `512@2x` = 1024 pixels). After changing `velo.svg`, render them again, e.g. with [resvg](https://github.com/linebender/resvg):

```sh
for s in 32 128 256 512; do resvg -w $s -h $s assets/icons/velo.svg packaging/icons/velo-$s.png; done
resvg -w 1024 -h 1024 assets/icons/velo.svg 'packaging/icons/velo-512@2x.png'
```
