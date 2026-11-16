"""Convert the maintained import dashboard to Grafana v2 for this installation.

Run after editing tvx-skynet-dashboard.json. The v1 file retains datasource mapping;
the v2 export uses the PostgreSQL UID provided for the SOC Grafana instance.
"""
import copy
import json
import re
import math
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
    field_config = copy.deepcopy(panel.get('fieldConfig', {'defaults': {}, 'overrides': []}))
    # These queries produce nonnegative status codes, counts and percentages.
    # Grafana's code-editor schema requires a numeric base threshold; classic
    # dashboard exports use null to represent -Infinity.
    for step in field_config['defaults'].get('thresholds', {}).get('steps', []):
        if step['value'] is None:
            step['value'] = 0
    elements[f"panel-{panel['id']}"] = {'kind': 'Panel', 'spec': {
        'id': panel['id'], 'title': panel['title'], 'description': panel.get('description', ''), 'links': panel.get('links', []), 'data': data,
        'vizConfig': {'kind': 'VizConfig', 'group': panel['type'], 'version': '13.2.4', 'spec': {
            'fieldConfig': field_config, 'options': panel.get('options', {})}}}}
provider_count = len(re.findall(r"^  - id:", (root / "config/sources.yaml").read_text(), re.MULTILINE))
network_count = len(re.findall(r"^    category: carriers", (root / "config/sources.yaml").read_text(), re.MULTILINE))
rows = []
# AutoGrid responds to the available width; incident text receives its own row.
for title, ids, columns, height, collapsed in [
    ('Overview', [1, 13], 2, 100, False),
    ('Service outages', [2], 1, 320, False),
    ('Network provider status', [18], 1, 100 + network_count * 44, False),
    ('Network outages', [17], 1, 640, False),
    ('All service providers', [4], 1, 100 + (provider_count - network_count) * 44, False),
    ('Collection reliability', [6], 1, 260, True),
    ('Incident details', [5], 1, 230, True),
    ('History', [7], 1, 360, True),
    ('Components', [8], 1, 300, True),
    ('Poll details', [9], 1, 260, True),
    ('Coverage', [10], 1, 280, True),
    ('Reading guide', [12], 1, 100, True),
]:
    items = [{'kind': 'AutoGridLayoutItem', 'spec': {'element': {
        'kind': 'ElementReference', 'name': f'panel-{panel_id}'}}} for panel_id in ids]
    rows.append({'kind': 'RowsLayoutRow', 'spec': {'title': title, 'collapse': collapsed,
        'layout': {'kind': 'AutoGridLayout', 'spec': {
            'maxColumnCount': columns, 'columnWidthMode': 'standard',
            'rowHeightMode': 'custom', 'rowHeight': height, 'fillScreen': False, 'items': items}}}})
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
