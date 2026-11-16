"""Convert the maintained import dashboard to Grafana v2 for this installation.

Run after editing tvx-skynet-dashboard.json. The v1 file retains datasource mapping;
the v2 export uses the PostgreSQL UID provided for the SOC Grafana instance.
"""
import copy
import json
from pathlib import Path

root = Path(__file__).resolve().parents[1]
folder = root / 'deploy/grafana'
source = json.loads((folder / 'tvx-skynet-dashboard.json').read_text())
datasource = {'name': 'dg46qijl86fi8d'}
elements = {}
for panel in source['panels']:
    data = {'kind': 'QueryGroup', 'spec': {'queries': [], 'queryOptions': {}, 'transformations': []}}
    for target in panel.get('targets', []):
        query = {k: v for k, v in target.items() if k not in ['datasource', 'refId', 'hide']}
        data['spec']['queries'].append({'kind': 'PanelQuery', 'spec': {'refId': target['refId'], 'hidden': target.get('hide', False), 'query': {
            'kind': 'DataQuery', 'group': 'grafana-postgresql-datasource', 'version': 'v0', 'datasource': datasource, 'spec': query}}})
    elements[f"panel-{panel['id']}"] = {'kind': 'Panel', 'spec': {
        'id': panel['id'], 'title': panel['title'], 'description': panel.get('description', ''), 'links': panel.get('links', []), 'data': data,
        'vizConfig': {'kind': 'VizConfig', 'group': panel['type'], 'version': '13.2.4', 'spec': {
            'fieldConfig': panel.get('fieldConfig', {'defaults': {}, 'overrides': []}), 'options': panel.get('options', {})}}}}
rows = []
for title, ids in [('Overview', [12, 1, 11, 2]), ('Provider state and coverage', [4, 10]), ('Incidents and collector health', [5, 6]), ('History and details', [7, 8, 9])]:
    panels = [p for p in source['panels'] if p['id'] in ids]
    top = min(p['gridPos']['y'] for p in panels)
    items = [{'kind': 'GridLayoutItem', 'spec': {'element': {'kind': 'ElementReference', 'name': f"panel-{p['id']}"},
        'x': p['gridPos']['x'], 'y': p['gridPos']['y'] - top, 'width': p['gridPos']['w'], 'height': p['gridPos']['h']}} for p in panels]
    rows.append({'kind': 'RowsLayoutRow', 'spec': {'title': title, 'collapse': False, 'layout': {'kind': 'GridLayout', 'spec': {'items': items}}}})
variables = []
for variable in source['templating']['list']:
    spec = copy.deepcopy(variable)
    spec.pop('type', None)
    spec['hide'] = 'dontHide'
    if spec.get('refresh') is not None:
        spec['refresh'] = {0: 'never', 1: 'onDashboardLoad', 2: 'onTimeRangeChanged'}[spec['refresh']]
    spec['sort'] = 'alphabeticalAsc'
    spec.pop('datasource', None)
    spec['allowCustomValue'] = True
    if isinstance(spec['query'], dict):
        spec['query'] = spec['query']['rawSql']
    spec['query'] = {'kind': 'DataQuery', 'group': 'grafana-postgresql-datasource', 'version': 'v0',
        'datasource': datasource, 'spec': {'rawSql': spec['query'], 'format': 'table', 'rawQuery': True, 'editorMode': 'code'}}
    variables.append({'kind': 'QueryVariable', 'spec': spec})
result = {'apiVersion': 'dashboard.grafana.app/v2', 'kind': 'Dashboard', 'metadata': {'name': 'tvx-skynet-external-status'}, 'spec': {
    'title': source['title'], 'description': source['description'], 'editable': True, 'cursorSync': 'Crosshair', 'tags': source['tags'],
    'annotations': [{'kind': 'AnnotationQuery', 'spec': {'builtIn': True, 'enable': True, 'hide': True,
        'iconColor': 'rgba(0, 211, 255, 1)', 'name': 'Annotations & Alerts',
        'query': {'kind': 'DataQuery', 'group': 'grafana', 'version': 'v0', 'datasource': {'name': '-- Grafana --'}, 'spec': {}}}}], 'links': [], 'liveNow': False, 'preload': False, 'elements': elements, 'layout': {'kind': 'RowsLayout', 'spec': {'rows': rows}},
    'timeSettings': {'from': source['time']['from'], 'to': source['time']['to'], 'timezone': 'browser', 'autoRefresh': source['refresh'],
        'autoRefreshIntervals': ['30s', '1m', '5m', '15m', '30m', '1h'], 'fiscalYearStartMonth': 0, 'hideTimepicker': False, 'weekStart': 'monday'},
    'variables': variables}}
(folder / 'tvx-skynet-dashboard-v2.json').write_text(json.dumps(result, indent=2) + '\n')
