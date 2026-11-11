"""Exercise Windows file locks and redirected folders with real exports."""
import argparse
import ctypes
import json
import pathlib
import subprocess
from ctypes import wintypes
from PIL import Image
from verify import recipe, run_job, monitor, sha, verify_exports

p = argparse.ArgumentParser()
p.add_argument('--exe', type=pathlib.Path, required=True)
p.add_argument('--work', type=pathlib.Path, required=True)
a = p.parse_args()
root = a.work.resolve()
source = root / 'source'
outside = root / 'outside'
source.mkdir(parents=True)
outside.mkdir()
for name in ['readable', 'locked']:
    Image.new('RGB', (120, 80), (40, 90, 130)).save(source / (name + '.jpg'))
Image.new('RGB', (120, 80), (80, 20, 100)).save(outside / 'must-not-follow.jpg')
before = {f.name: sha(f) for f in source.glob('*.jpg')}

def junction(link, target):
    # Pass paths as environment values, not interpolated PowerShell code.
    import os
    env = os.environ.copy()
    env.update(FRAMEBATCH_LINK=str(link), FRAMEBATCH_TARGET=str(target))
    subprocess.run(['powershell', '-NoProfile', '-Command',
                    'New-Item -ItemType Junction -Path $env:FRAMEBATCH_LINK -Target $env:FRAMEBATCH_TARGET | Out-Null'],
                   env=env, check=True, capture_output=True)

junction(source / 'nested-link', outside)
junction(root / 'redirected-source', source)
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                              ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
handle = kernel.CreateFileW(str(source / 'locked.jpg'), 0x80000000, 0, None, 3, 0x80, None)
assert handle != ctypes.c_void_p(-1).value, ctypes.get_last_error()
try:
    r = recipe(root / 'redirected-source', root / 'output')
    r['size']['resize'] = False
    first = run_job(a.exe.resolve(), r, root, 'locked-run')
    assert first['final']['discovered'] == 2, first
    assert first['final']['completed'] == 1 and first['final']['failed'] == 1, first
finally:
    kernel.CloseHandle(handle)
resumed = monitor([str(a.exe.resolve()), 'resume', first['journal'], '--retry'], root / 'retry.jsonl')
assert resumed['exit_code'] == 0 and resumed['final']['completed'] == 2 and resumed['final']['failed'] == 0, resumed
checks = verify_exports(source, root / 'output', 2)
assert before == {f.name: sha(f) for f in source.glob('*.jpg')}
(root / 'verification.json').write_text(json.dumps(dict(first=first, resumed=resumed,
    independent_reader=checks, nested_junction_skipped=True, redirected_source_resolved=True,
    source_hashes_unchanged=True), indent=2), encoding='utf-8')
print('PASS: locked input reported and retried, explicit redirected source resolved, nested junction skipped, originals unchanged')
