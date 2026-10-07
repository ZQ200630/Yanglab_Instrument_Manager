"""Published Velocity envelopes, not head calibration or emission limits.

Table: SP-NF-DS-20171024-Velocity6700.pdf, supplied by Newport.
-P: permanent fiber coupling, New-Focus-Tunable-Diode-Lasers-Brochure.pdf.
Unknown/custom suffixes never inherit the standard envelope.
"""
import re

import json
from pathlib import Path

SPECS = json.loads(Path(__file__).with_suffix('.json').read_text(encoding='utf-8'))
RANGES = {model:tuple(spec[:2]) for model,spec in SPECS.items()}

def head_spec(head):
    match=re.fullmatch(r'(?:TLB-)?([0-9]{4})(?:-P)?',head)
    return tuple(SPECS[match[1]]) if match and match[1] in SPECS else None
