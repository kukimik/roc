app [main] { pf: platform "https://github.com/roc-lang/basic-cli/releases/download/0.10.0/vNe6s9hWzoTZtFmNkvEICPErI9ptji_ySjicO6CkucY.tar.br" }

import pf.Stdout
import "../../LICENSE" as license

main =
    license
        |> List.map Num.toU64
        |> List.sum
        |> Num.toStr
        |> Stdout.line!
