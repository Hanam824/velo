# Fixtures

Hand-written `/youtubei/v1/player` answers with the same shape as YouTube's, trimmed to the
fields Velo reads. None of them is a capture of a real video; the addresses point at
`media.test`, which doesn't exist.

- `base.js` imitates the layout of the web player script so the signature and n-function
  searches can be tested; it is written for these tests, not taken from YouTube.
- `iframe_api` imitates the script that names the current player version.
