"""Check export structure, datasource references, panel IDs and layout coverage."""
import json
from pathlib import Path
root = Path(__file__).resolve().parents[1] / 'deploy/grafana'
v1 = json.loads((root / 'tvx-skynet-dashboard.json').read_text())
v2 = json.loads((root / 'tvx-skynet-dashboard-v2.json').read_text())
assert v2['apiVersion'] == 'dashboard.grafana.app/v2'
spec = v2['spec']
ids = [p['spec']['id'] for p in spec['elements'].values()]
assert len(ids) == len(set(ids)) == len(v1['panels']) == 11
references = []
for row in spec['layout']['spec']['rows']:
    layout = row['spec']['layout']
    assert layout['kind'] == 'AutoGridLayout', 'panels must resize with available width'
    assert layout['spec']['maxColumnCount'] in (1, 2)
    for item in layout['spec']['items']:
        assert item['kind'] == 'AutoGridLayoutItem'
        references.append(item['spec']['element']['name'])
assert sorted(references) == sorted(spec['elements'])
for panel in spec['elements'].values():
    p = panel['spec']
    assert p['description']
    assert {'links', 'vizConfig', 'data'} <= p.keys()
    # Grafana's Edit as code validator requires numbers here, even though
    # classic exports serialize the base threshold as null (-Infinity).
    for index, step in enumerate(p['vizConfig']['spec']['fieldConfig']['defaults'].get('thresholds', {}).get('steps', [])):
        assert type(step['value']) in (int, float), f"panel-{p['id']} threshold {index}: expected number, got {step['value']!r}"
    for query in p['data']['spec']['queries']:
        data = query['spec']['query']
        assert data['kind'] == 'DataQuery'
        assert data['datasource']['name'] == 'dg46qijl86fi8d'
        assert data['spec']['rawSql']
for variable in spec['variables']:
    v = variable['spec']
    assert {'name', 'current', 'hide', 'refresh', 'skipUrlSync', 'query', 'regex', 'sort', 'options', 'multi', 'includeAll', 'allowCustomValue'} <= v.keys()
    assert v['query']['kind'] == 'DataQuery'
    assert v['query']['datasource']['name'] == 'dg46qijl86fi8d'
print('Both JSON exports parse; 11 unique panels, complete layouts and correct v2 query/datasource structure.')
