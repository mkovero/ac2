One module per area (e.g. `mtw.py`, `spl.py`). Each exports `GENERATORS = [fn, ...]`; each
`fn()` returns a `VectorSet`. Import helpers with:

```python
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import generate as g  # g.VectorSet, g.lin_tol, g.db_tol, g.periodic_hann, ...
```
