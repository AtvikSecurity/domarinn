# Evaluate structured output

**The problem.** An agent must emit a structured object — a verdict per catalogue item, a set of extracted fields, a report with required sections. "Did it produce valid JSON" and "did it produce the *right* JSON" are different questions, and only the first one is easy.

**The shape.** Stack three layers, cheapest first: parseability, then shape, then semantics.

## 1. Parseability and shape, for free

```yaml
--8<-- "examples/04-json-output/domarinn.yaml"
```

`is-json` asks whether the *whole* output parses. `contains-json` looks for an object embedded anywhere in it — which is what you want when the model wraps its answer in prose you cannot fully suppress.

Give `contains-json` a schema, and give the schema a `required` list. Without one, a model that returns `{}` passes:

```yaml
- type: contains-json
  schema:
    type: object
    required: ["verdicts", "summary"]
    properties:
      verdicts: { type: array }
      summary: { type: string }
```

That is a real gate and it costs nothing. Most structured-output bugs die here.

## 2. Semantics, with a rubric over specific fields

Schema validation cannot tell you that an item was marked `TESTED` when the evidence says otherwise. A rubric can — provided you point it at named fields rather than at "the JSON":

```yaml
- type: contains-json
  schema: { type: object, required: ["verdicts"] }
- type: llm-rubric
  value: |
    The JSON `verdicts` array MUST contain an entry for every id in the input
    catalogue, and no others.

    An item with no recorded activity MUST be marked `MISSED`, never `TESTED`.

    Score 0 if any item with zero recorded activity is marked `TESTED`, or if
    an id appears that was not in the catalogue.

    Do NOT penalise the wording of `summary`, the ordering of `verdicts`, or
    any additional field the object carries.
```

Note the ordering: `contains-json` runs first and short-circuits, so a model that emitted nothing parseable never reaches the grader.

## 3. Include the negative case

The case most suites are missing is the one where the correct answer is **not** the confident one — an item that must be marked "not tested" despite a misleading upstream signal. Those are the cases that catch a model optimising for looking complete.

## 4. Pass JSON inputs as string vars

When a case's input is itself structured, pass it as a JSON *string* var:

```yaml
vars:
  catalog: '[{"id":"a","applies_when":"HTTP endpoints present"}]'
  activity: '{"a":{"row_count":0}}'
```

/// warning | Watch for template syntax in the payload

A JSON fixture containing `{{` will be rendered before it reaches the system. If the payload must arrive byte-for-byte, tag it [`!raw`](../examples/templates-and-test-data.md#example-06--the-raw-escape-hatch) — or load it from a file with `{$file: "…", raw: true}`, as [example 07](../examples/templates-and-test-data.md#example-07--file-content-vars) does.

///

## See also

- [Example 04](../examples/first-steps.md#example-04--structured-output) — the parseability layer.
- [Example 29](../examples/models-grading-and-budgets.md#example-29--llm-rubric-grading) — writing the rubric.
- [Assertions](../reference/assertions.md) — `contains-json` and schema handling in full.
