"""Print the differences between two runs of wb_design_measure.py (measure.json files)."""
import json, sys
A = json.load(open(sys.argv[1], encoding="utf-8"))
B = json.load(open(sys.argv[2], encoding="utf-8"))
SKIP = {"sample", "n"}
diffs = 0
def walk(a, b, path):
    global diffs
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k in SKIP:
                continue
            if k not in a or k not in b:
                print("ONLY ONE SIDE", path + "/" + k); diffs += 1; continue
            walk(a[k], b[k], path + "/" + k)
    elif a != b:
        if isinstance(a, list) and isinstance(b, list) and all(isinstance(x, dict) for x in a + b):
            ka = {x.get("el"): x for x in a}; kb = {x.get("el"): x for x in b}
            for k in sorted(set(ka) | set(kb), key=str):
                if ka.get(k) != kb.get(k):
                    print("DIFF", path, k, ka.get(k), "->", kb.get(k)); diffs += 1
            return
        print("DIFF", path, a, "->", b); diffs += 1
walk(A, B, "")
print("diffs:", diffs)
