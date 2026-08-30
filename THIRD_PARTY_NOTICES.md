# Third-Party Notices

This document records third-party material distributed in this repository.
It does not select or grant a license for Haven itself. Haven's own source is
licensed under the MIT License in the repository root; third-party material
below remains governed by its respective license and notice obligations.

## Live2D widget model packages

The following npm packages are vendored under
`前端/app/public/live2d/models/<model>/assets/`:

| Package | Version | npm SHA-1 | npm SHA-512 integrity |
|---|---:|---|---|
| `live2d-widget-model-miku` | 1.0.5 | `49a0b7b4bbc371e36b75cf62d745933e7bcf108b` | `sha512-uwthpWUAeqiA/tL87QNri1eNp0w0UKxM34n/PhA0ng/ZU4hhsJKYHaRcoPK8oOWbpLYCoS8uzWAACmXfomylTg==` |
| `live2d-widget-model-shizuku` | 1.0.5 | `136f269690b20738fa0e9b67e33aa643007512a8` | `sha512-keEQR1Bm7HILLjNSIpmg1SGuo4H/ZsGRxnBn0l0BxgEbaHlmEE+Jb5nwhAtnh7PN/SRFE3+Ddk/ciy18sT7rAw==` |
| `live2d-widget-model-koharu` | 1.0.5 | `6525e2c495af40b074d91c31499d4a71711cfccc` | `sha512-tziYYbBvkLFbYmizb6Sij4rF1Vmne9ZJwwD8dljt3QPeWxYBjxuQspzLrFOjMtvCSeE5dncNG7qdnTUb5nD2DQ==` |
| `live2d-widget-model-unitychan` | 1.0.5 | `fdf0ad303eaca04d42dec05c45a82a24184b7953` | `sha512-7mkTW/VFYdBqA8vjCpn2x3mZoqcdx35LqSEAVmsr/pDnEOhmm8GFOCNpwYOybvg+oBbB3fXeKdLHtv0q7SU9pA==` |
| `live2d-widget-model-z16` | 1.0.5 | `7604025e733d6fc6b24a60c3231e5b62d5e532a1` | `sha512-1CUY6iOWakrJQdShWOJwzYvM0rj0Ok71B5qbKT03/ATPKUYmDLgSIy4QiH9CDZkXSwlQtIEG7CUqqaUzFZTB4w==` |

- Publisher-declared license: `GPL-2.0`
- Upstream repository: <https://github.com/xiazeyu/live2d-widget-models>
- Registry source: <https://www.npmjs.com/search?q=live2d-widget-model>
- License text: [`third_party/licenses/GPL-2.0.txt`](third_party/licenses/GPL-2.0.txt)
- Local modification: package asset bytes are unchanged; only their containing
  directory was relocated for offline loading.

The package metadata and upstream repository license identify GPL-2.0 for the
published packages. Names, character designs, trademarks, and other rights can
require separate review and are not granted by this notice. Public distribution
therefore remains subject to Owner/legal review.

## oh-my-live2d runtime component

Haven uses `oh-my-live2d` version `0.19.3` to host the local Live2D models.

- Package integrity: `sha512-EWva8OUmcExIFIhqGkeiVPQt44RvYYFg+nmpkrZgQVQ+xofVFq4SDYAg5NcdK40HxyV4vORBJEg4OHIHP96MrA==`
- Upstream repository: <https://github.com/oh-my-live2d/oh-my-live2d>
- Publisher-declared license: `MIT`
- License text: [`third_party/licenses/oh-my-live2d-MIT.txt`](third_party/licenses/oh-my-live2d-MIT.txt)
- Local modification: Haven's Vite build removes the package's inline Cubism
  script injection, dynamic style injection, remote version check, and remote
  daily-tip fetch. The locked Cubism runtime and reviewed global CSS are emitted
  as same-origin external assets so the desktop app can retain a strict Content
  Security Policy and work offline.

## Pixi CSP-safe ShaderSystem implementation

Haven uses the CSP-safe ShaderSystem implementation from `@pixi/unsafe-eval`
version `6.5.10` to replace the dynamic `Function` path in the Pixi 6.5.10 copy
embedded by `oh-my-live2d`.

- Package integrity: `sha512-IC/SjQb4vXBgtcLeERjmgb55tdFVJizBQJLytaKFSlJk9gQf3itGmwx4yF/S3+ArrJpJdYrKSR9DB8XNw8oK/w==`
- Upstream repository: <https://github.com/pixijs/pixi.js>
- Publisher-declared license: `MIT`
- License text: [`third_party/licenses/pixi-unsafe-eval-MIT.txt`](third_party/licenses/pixi-unsafe-eval-MIT.txt)
- Local modification: Haven's Vite build installs this implementation into the
  locked embedded Pixi ShaderSystem. The build fails closed if either package
  version marker or the reviewed source layout changes.

## AI-generated local avatar images

The Human Owner confirmed on 2026-08-19 that the five JPEG files under
`前端/app/public/avatars/` are AI-generated project assets and are cleared for
distribution with Haven. Historical Unsplash URLs previously associated with
the UI were not the source of the checked-in bytes and are not the licensing
basis for these files.

| Local file | Provenance record | Current local SHA-256 |
|---|---|---|
| `miku.jpg` | Human Owner declaration, AI-generated project asset | `756b2f0630144763dcd89f1efea4e1b707df07d77b559e828c5c4abf57b8c8c1` |
| `shizuku.jpg` | Human Owner declaration, AI-generated project asset | `fb6479cbfa6e5f63c07e6c8f30a6bbea28d7ff74b5f61fc2194cb072b564e2e2` |
| `koharu.jpg` | Human Owner declaration, AI-generated project asset | `bcee89b7510f1badbc501d5476ea22db8deb2f01b14d9723f5e4e2dd471540e5` |
| `unitychan.jpg` | Human Owner declaration, AI-generated project asset | `782043d662e0253112cc45f601b9f7b7c836e08e890f221db8f865c8c91d5937` |
| `z16.jpg` | Human Owner declaration, AI-generated project asset | `1f11ad0fe3007ac4c283cf242ed16182cb4b9b20ab5127d0de636934545a871a` |

The release process must re-check these hashes against the packaged assets.
