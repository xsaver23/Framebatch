"""Semantic checks on actual lossless exports, independent of the Rust pixel reader.
These validate control behavior; they do not establish missing prototype parity.
"""
import argparse
import json
import pathlib
import numpy as np
from PIL import Image
from verify import recipe, run_job, sha

p = argparse.ArgumentParser()
p.add_argument('--exe', type=pathlib.Path, required=True)
p.add_argument('--work', type=pathlib.Path, required=True)
a = p.parse_args()
root = a.work.resolve()
source = root / 'input'
source.mkdir(parents=True, exist_ok=True)
Image.new('RGB', (64, 64), (60, 100, 140)).save(source / 'color.png')
gradient = np.broadcast_to(np.linspace(32, 224, 64, dtype=np.uint8)[None, :, None], (64, 64, 3)).copy()
Image.fromarray(gradient).save(source / 'gradient.png')
edge = Image.new('RGB', (64, 64), (50, 50, 50))
edge.paste((180, 180, 180), (32, 0, 64, 64))
edge.save(source / 'edge.png')
# Neutral gradient with a warm cast (+16 red, -16 blue): what Auto Color should remove.
cast = gradient + np.array([16, 0, -16])
Image.fromarray(cast.astype(np.uint8)).save(source / 'cast.png')
before = {f.name: sha(f) for f in source.iterdir()}
results = {}

def export(name, manual=None, quick=None):
    r = recipe(source, root/name/'output', format='png', quick=quick or {}, manual=manual or {})
    r['size']['resize'] = False
    result = run_job(a.exe.resolve(), r, root, name)
    assert result['exit_code'] == 0 and result['final']['completed'] == len(before), result
    images = {}
    for f in (root/name/'output').glob('*.png'):
        with Image.open(f) as im:
            im.load()
            assert all(abs(v-300) < .02 for v in im.info['dpi'])
            images[f.stem.removesuffix('-edited')] = np.asarray(im.convert('RGB')).astype(int)
    results[name] = result
    return images

neutral = export('neutral')
assert np.array_equal(neutral['gradient'], gradient)
assert (neutral['color'] == [60, 100, 140]).all()
exposure = export('exposure', {'exposure': 1})['color']
assert (exposure > neutral['color']).all()
contrast = export('contrast', {'contrast': 50})['gradient']
assert contrast[32,0,0] < 32 and contrast[32,-1,0] > 224
shadows = export('shadows', {'shadows': 50})['gradient'] - neutral['gradient']
assert shadows[32,0,0] > shadows[32,-1,0] >= 0
highlights = export('highlights', {'highlights': 50})['gradient'] - neutral['gradient']
assert highlights[32,-1,0] > highlights[32,0,0] >= 0
saturation = export('saturation', {'saturation': -100})['color']
assert np.max(saturation, axis=2).max() - np.min(saturation, axis=2).min() <= 1
warmth = export('warmth', {'warmth': 50})['color'][0,0]
assert warmth[0] > 60 and warmth[1] == 100 and warmth[2] < 140
sharp = export('sharpening', {'sharpening': 100})['edge']
assert sharp[32,31,0] < 50 and sharp[32,32,0] > 180
black = export('black-point', {'black_point': 60})['color'][0,0]
assert black[0] == 0 and black[2] > black[1] > 0
white = export('white-point', {'white_point': 140})['color'][0,0]
assert white[2] == 255 and 0 < white[0] < white[1]
gamma = export('gamma', {'gamma': 2})['color']
assert (gamma > neutral['color']).all()
mid = lambda im: im[32, 32].astype(int)
for flag in ['levels', 'contrast']:
    fixed = export('auto-'+flag, quick={flag: True})
    assert fixed['gradient'][32,0,0] < 5 and fixed['gradient'][32,-1,0] > 250
    # Flat images are left alone (Adobe: no improvement for flat-color images).
    assert (np.abs(fixed['color'] - [60, 100, 140]) <= 1).all(), fixed['color'][0,0]
    if flag == 'levels':  # per-channel stretch removes the cast
        assert np.ptp(mid(fixed['cast'])) <= 3, mid(fixed['cast'])
    else:  # one shared mapping keeps (and scales) the cast
        assert mid(fixed['cast'])[0] - mid(fixed['cast'])[2] >= 32, mid(fixed['cast'])
color = export('auto-color', quick={'color': True})
# Auto Color neutralizes a cast gradient but keeps a genuinely colored flat image.
assert np.ptp(mid(color['cast'])) <= 3, mid(color['cast'])
assert (np.abs(color['color'] - [60, 100, 140]) <= 1).all(), color['color'][0,0]
fixed = export('auto-fix', quick=dict(levels=True,contrast=True,color=True,sharpen=True))
assert (np.abs(fixed['color'] - [60, 100, 140]) <= 2).all(), fixed['color'][0,0]
assert np.ptp(mid(fixed['cast'])) <= 3, mid(fixed['cast'])
assert before == {f.name: sha(f) for f in source.iterdir()}
(root/'verification.json').write_text(json.dumps(results,indent=2),encoding='utf-8')
print('PASS: neutral reset, all ten manual controls, automatic corrections and unchanged sources')
