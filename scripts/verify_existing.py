"""Read-only independent verification of an installed-app Sony benchmark export.

Every file is reopened and hashed; each distinct encoded byte sequence is fully
decoded by Pillow. Repeated byte-identical files reuse that successful decode.
"""
import argparse
import json
import pathlib
from PIL import Image
from verify import sha

p = argparse.ArgumentParser()
p.add_argument('--source', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--report', type=pathlib.Path, required=True)
p.add_argument('--count', type=int, required=True)
p.add_argument('--ppi', type=float, default=300)
p.add_argument('--corpus-manifest', type=pathlib.Path, required=True)
p.add_argument('--reference-output', type=pathlib.Path, help='Previously verified output with the identical recipe and engine; compare repeated inputs byte-for-byte')
a = p.parse_args()
sources = sorted(f for f in a.source.iterdir() if f.suffix.lower() in ['.jpg', '.jpeg'])
outputs = sorted(a.output.glob('*.jpg'))
assert len(sources) == len(outputs) == a.count, (len(sources), len(outputs), a.count)
assert not list(a.output.glob('*.part')), 'Unfinished output files remain'
manifest = {row['name']: row['sha256'] for row in json.loads(a.corpus_manifest.read_text(encoding='utf-8'))}
known = set()
source_hashes = {}
reference_hashes = {}
reference_mismatches = []
for src in sources:
    original_name = src.name.split('-', 1)[1]
    dest = a.output / (src.stem + '-edited.jpg')
    assert dest.exists(), dest
    with Image.open(src) as original:
        expected = original.size
        if original.getexif().get(274, 1) in [5, 6, 7, 8]:
            expected = expected[::-1]
    digest = sha(dest)
    if a.reference_output:
        if original_name not in reference_hashes:
            matches = sorted(a.reference_output.glob('*-' + pathlib.Path(original_name).stem + '-edited.jpg'))
            assert matches, ('Missing reference output', original_name)
            reference_hashes[original_name] = sha(matches[0])
        if digest != reference_hashes[original_name]:
            reference_mismatches.append(dict(file=str(dest), sha256=digest, reference_sha256=reference_hashes[original_name]))
    with Image.open(dest) as im:
        if digest not in known:
            im.load()
            known.add(digest)
        assert im.size == expected, (dest, im.size, expected)
        assert im.info.get('dpi') == (a.ppi, a.ppi), dest
        assert im.getexif().get(274, 1) == 1, dest
        assert not im.getexif().get(34853), dest
        assert im.info.get('icc_profile'), dest
    # Read every input as well; repeated originals are checked against the saved corpus manifest separately.
    source_hashes[src.name] = sha(src)
    assert source_hashes[src.name] == manifest[original_name], ('Source changed', src)
result = dict(count=a.count, header_checks=len(outputs), sha256_verified=len(outputs),
              distinct_contents_fully_decoded=len(known), dimensions='source pixels, oriented',
              original_hashes_match_manifest=True, ppi=a.ppi, orientation=1, icc_present=True, gps_absent=True,
              source_sha256=source_hashes)
if a.reference_output:
    result['identical_to_verified_reference'] = not reference_mismatches
    result['reference_output'] = str(a.reference_output.resolve())
    result['reference_mismatches'] = reference_mismatches
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
print(json.dumps({k:v for k,v in result.items() if k != 'source_sha256'}))
assert not reference_mismatches, f'{len(reference_mismatches)} exports differ from the verified reference; see report'
