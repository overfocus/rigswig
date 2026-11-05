# Fonts for the page

kevinferron.com's two faces. `rigswig ui` builds them into the binary (`include_bytes!` in `src/ui.rs`) and inlines them
in the page as `data:` fonts (`font-src data:` in the CSP), so the page never fetches a font from anywhere.

| File | Family | Licence | Upstream path |
|---|---|---|---|
| `JosefinSans-wght.ttf` | Josefin Sans (variable, weight 100 to 700) | SIL OFL 1.1, **Reserved Font Name "Josefin Sans"** (`OFL-JosefinSans.txt`) | `ofl/josefinsans/JosefinSans[wght].ttf` |
| `PixelifySans-wght.ttf` | Pixelify Sans (variable, weight 400 to 700) | SIL OFL 1.1, no Reserved Font Name (`OFL-PixelifySans.txt`) | `ofl/pixelifysans/PixelifySans[wght].ttf` |

Source: https://github.com/google/fonts at commit `eb20e9cf19d97cf579fa3867d729d4222d6c59e1` (read 2026-11-05), downloaded once by hand with `curl` from `raw.githubusercontent.com`.

**Both files are the unmodified upstream bytes** (`git hash-object` equals the blob id GitHub reports at that commit: `4d59aa52b1fa6235b5b917793233e8526cc7ed50` for Josefin, `2d7eb388ee499fbcf3e992723ec409c095640104` for Pixelify). Only the file names differ (the brackets are dropped). Josefin Sans carries a Reserved Font Name, so it must not be subset, converted (to WOFF2) or edited here; kevinferron.com's own `public/fonts/og/JosefinSans-*.ttf` are static instances cut from the variable font and are Modified Versions, so they are not used. If rigswig ever needs a modified Josefin, give it a new family name first (OFL section 3).

Identifiers are set in the system monospace.
