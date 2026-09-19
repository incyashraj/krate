#!/usr/bin/env python3
"""Save read-only discovery snapshots and compare them without overlapping totals.

No trackers, credentials, scheduled jobs, external writes or runtime telemetry
are added. Raw snapshots belong in a private local evidence directory.
"""
import argparse
import concurrent.futures
import csv
import datetime as dt
import json
from pathlib import Path
import subprocess
import sys

REPO = 'incyashraj/krate'
SCHEMA = 'krate.discovery.snapshot.v1'
ENDPOINTS = {'repository':'', 'views':'/traffic/views', 'clones':'/traffic/clones',
             'referrers':'/traffic/popular/referrers', 'paths':'/traffic/popular/paths'}

def github(suffix):
    result = subprocess.run(['gh', 'api', 'repos/'+REPO+suffix],
                            capture_output=True, text=True, timeout=45)
    if result.returncode:
        raise RuntimeError(f'GitHub read unavailable (exit {result.returncode})')
    return json.loads(result.stdout)

def read_source(name):
    try:
        if name == 'hub':
            # Use the platform curl trust store (not a Python-specific CA
            # bundle). Certificate verification stays on; never use --insecure.
            result = subprocess.run(['curl','--fail','--silent','--show-error',
                '--max-time','45','https://hub.krate.tech/stats'],
                capture_output=True,text=True,timeout=50)
            if result.returncode:
                raise RuntimeError(f'Hub read unavailable (curl exit {result.returncode})')
            data = json.loads(result.stdout)
        else:
            data = github(ENDPOINTS[name])
        if name == 'repository':
            data = {key:data.get(key) for key in ['full_name','description','homepage',
                    'stargazers_count','forks_count','subscribers_count','topics']}
        return name, {'available':True, 'data':data}
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        return name, {'available':False, 'error':str(error), 'data':None}

def capture():
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        sources = dict(pool.map(read_source, [*ENDPOINTS, 'hub']))
    return {'schema':SCHEMA, 'captured_at':dt.datetime.now(dt.timezone.utc).isoformat(),
            'repository':REPO, 'sources':sources}

def data(snapshot, source):
    entry = snapshot.get('sources', {}).get(source, {})
    return entry.get('data') if entry.get('available') else None

def daily_rows(snapshots):
    """Newest successful observation wins for each source/day, never sum windows."""
    rows = {}
    for snapshot in sorted(snapshots, key=lambda s:s['captured_at']):
        for source, collection in [('views','views'), ('clones','clones')]:
            body = data(snapshot, source)
            if not isinstance(body, dict):
                continue
            for item in body.get(collection, []):
                day = item['timestamp'][:10]
                row = rows.setdefault(day, {'date':day})
                row['github_'+source] = item.get('count')
                row['github_'+source+'_daily_uniques'] = item.get('uniques')
                row['github_'+source+'_observed_at'] = snapshot['captured_at']
        body = data(snapshot, 'hub')
        live = (body or {}).get('live') or {}
        if not isinstance(live.get('actions_by_day'), dict):
            continue
        for day, actions in live['actions_by_day'].items():
            row = rows.setdefault(day, {'date':day})
            for key in ['view','install','make','open','publish']:
                row['hub_'+key+'_events'] = actions.get(key, 0)
            row['hub_observed_at'] = snapshot['captured_at']
    return [rows[key] for key in sorted(rows)]

def markdown(snapshots):
    latest = max(snapshots, key=lambda s:s['captured_at'])
    repo = data(latest, 'repository') or {}
    views = data(latest, 'views') or {}
    clones = data(latest, 'clones') or {}
    show = lambda value: 'Unknown' if value is None else str(value)
    lines = ['# Krate discovery snapshot', '', 'Observed: '+latest['captured_at'], '',
             '| Metric | Latest observation |', '| --- | --- |',
             '| GitHub stars | '+show(repo.get('stargazers_count'))+' |',
             '| GitHub views, returned rolling window | '+show(views.get('count'))+' |',
             '| GitHub unique visitors, returned rolling window | '+show(views.get('uniques'))+' |',
             '| GitHub clones, returned rolling window | '+show(clones.get('count'))+' |',
             '| Google search impressions / clicks | Unknown: export needed |',
             '| Bing search impressions / clicks | Unknown: export needed |',
             '| External developers activated / apps shipped | Unknown: direct confirmation needed |', '',
             'GitHub traffic windows overlap. Do not add their totals. Daily rows below use',
             'the newest successful observation for each source and date; daily unique',
             'counts must not be summed into a unique-person total. Today may be partial.',
             'Clones and hub activity include development, bots and CI, not just users.', '',
             '## Daily observations', '',
             '| UTC date | GitHub views | Daily unique visitors | Hub page-view events |',
             '| --- | --- | --- | --- |']
    for row in daily_rows(snapshots):
        lines.append('| '+' | '.join([row['date'],show(row.get('github_views')),
                     show(row.get('github_views_daily_uniques')),show(row.get('hub_view_events'))])+' |')
    missing = [name for name,entry in latest['sources'].items() if not entry['available']]
    if missing:
        lines += ['', 'Unavailable source reads: '+', '.join(missing)+'. Older daily observations remain dated.']
    return '\n'.join(lines)+'\n'

def self_test():
    import unittest
    class Tests(unittest.TestCase):
        def snap(self, stamp, count=2, uniques=1):
            return {'schema':SCHEMA,'captured_at':stamp,'sources':{'views':{'available':True,'data':{
                'count':count,'uniques':uniques,'views':[{'timestamp':'2026-09-18T00:00:00Z','count':count,'uniques':uniques}]}}}}
        def test_overlap_uses_latest(self):
            result=daily_rows([self.snap('2026-09-19',5),self.snap('2026-09-18',2)])
            self.assertEqual(result[0]['github_views'],5)
            self.assertEqual(len(result),1)
        def test_daily_uniques_not_added(self):
            self.assertEqual(daily_rows([self.snap('2026-09-18'),self.snap('2026-09-19')])[0]['github_views_daily_uniques'],1)
        def test_missing_is_unknown(self):
            self.assertIn('Unknown',markdown([{'schema':SCHEMA,'captured_at':'2026-09-19','sources':{}}]))
        def test_zero_is_real_zero(self):
            self.assertIn('| GitHub views, returned rolling window | 0 |',markdown([self.snap('2026-09-19',0,0)]))
        def test_failed_read_does_not_erase_history(self):
            failed={'schema':SCHEMA,'captured_at':'2026-09-20','sources':{'views':{'available':False,'data':None}}}
            self.assertEqual(daily_rows([self.snap('2026-09-19'),failed])[0]['github_views'],2)
        def test_legacy_hub_not_fresh(self):
            s=self.snap('2026-09-19');s['sources']['hub']={'available':True,'data':{'live':{'error':'unavailable'},'actions_by_day':{'2026-09-18':{'view':999}}}}
            self.assertNotIn('hub_view_events',daily_rows([s])[0])
    result=unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests))
    return 0 if result.wasSuccessful() else 1

def main():
    if '--self-test' in sys.argv:
        return self_test()
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest='command',required=True)
    c=commands.add_parser('capture'); c.add_argument('--output-dir',type=Path,required=True)
    r=commands.add_parser('report'); r.add_argument('snapshots',type=Path,nargs='+'); r.add_argument('--csv',action='store_true')
    args=parser.parse_args()
    if args.command=='capture':
        snapshot=capture(); args.output_dir.mkdir(parents=True,exist_ok=True)
        name='discovery-'+dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')+'.json'
        destination=args.output_dir/name
        with destination.open('x') as out: json.dump(snapshot,out,indent=2);out.write('\n')
        print(destination)
        return 0 if all(e['available'] for e in snapshot['sources'].values()) else 2
    snapshots=[json.loads(p.read_text()) for p in args.snapshots]
    if any(s.get('schema')!=SCHEMA or s.get('repository',REPO)!=REPO for s in snapshots):
        parser.error('Snapshot schema or repository mismatch')
    if args.csv:
        rows=daily_rows(snapshots)
        fields=['date']+sorted({k for row in rows for k in row if k!='date'})
        writer=csv.DictWriter(sys.stdout,fieldnames=fields);writer.writeheader();writer.writerows(rows)
    else:
        print(markdown(snapshots),end='')
    return 0

if __name__=='__main__':
    raise SystemExit(main())
