#!/usr/bin/env python3
"""Выдуманный контекст для карты знаний (ADR 0048): снимки панели и замер предела карты.

Ни одного настоящего имени: проект `supply-registry` вымышленной компании, пять документов о
её реестре поставщиков и одна беседа. Модели здесь нет — вместо `claude` на PATH стоит
скрипт, который отвечает заготовкой. Всё остальное настоящее: проход индексирует документы,
очередь `index jobs run` отправляет пронумерованные части «инструменту», программа проверяет
цитаты, склеивает сущности, пишет граф, а `knowledge map --json` читает его для панели.
Одна цитата в заготовках нарочно пересказана — её программа должна отвергнуть.

С `--scale N` вместо пяти документов пишутся N/6 документов по шесть фраз вида «Модуль 17
использует модуль 42.» о пуле из N сущностей; инструмент разбирает фразы сам. Это карта
заданного размера — по ней назван предел `MAP_MOST_ENTITIES`.

Запуск (из корня репозитория, ядро собрано `cargo build --release`):

    python3 apps/panel/tools/knowledge_sandbox.py <каталог> [--scale N]

Печатает переменные окружения, с которыми ядро и панель видят этот контекст.
"""
import json
import os
import random
import re
import stat
import subprocess
import sys

CORE = os.path.abspath("target/release/crosskit")

DOCUMENTS = {
    "registry.md": """# Реестр поставщиков

Реестр поставщиков хранит карточку каждого контрагента.
Контрагент уникален по БИН: две карточки с одним БИН — это дубль.
Карточка контрагента ссылается на договор поставки.
Филиал входит в головное юрлицо и имеет свой БИН.
""",
    "packages.md": """# Пакеты документов

Каждый пакет документов принадлежит одному контрагенту.
Пакет документов хранится в архиве сканов.
Акт сверки входит в пакет документов.
""",
    "state-check.md": """# Сверка с госреестром

Сверка с госреестром использует БИН карточки.
Ликвидированного контрагента заменяет правопреемник.
Сверка порождает отчёт о расхождениях.
""",
    "dedup-rule.md": """# Правило склейки дублей

Правило склейки регулирует реестр поставщиков.
Дубль карточки противоречит уникальности БИН.
""",
    "payments.md": """# Платёжный сервис

Сервис payments-api использует реестр поставщиков.
Платёжное поручение ссылается на договор поставки.
""",
}

# Что «модель» отвечает о каждом документе: сущности и связи с дословной цитатой. Ключ —
# фраза, по которой инструмент узнаёт документ в том, что ему прислали.
E = lambda name, kind, about="": {"name": name, "type": kind, "description": about}
R = lambda s, p, o, quote, weight=6: {"subject": s, "predicate": p, "object": o, "description": "",
                                      "weight": weight, "quote": quote}
ANSWERS = {
    "Реестр поставщиков хранит": ("реестр поставщиков", [
        E("Реестр поставщиков", "dataset", "Список карточек всех контрагентов."),
        E("Контрагент", "concept", "Сторона договора поставки."),
        E("Контрагенты", "concept"),
        E("БИН", "attribute", "Номер, по которому контрагент уникален."),
        E("Договор поставки", "document"),
        E("Филиал", "organization", "Подразделение юрлица со своим БИН."),
        E("Головное юрлицо", "organization"),
    ], [
        R("Реестр поставщиков", "has-attribute", "Контрагенты", "Реестр поставщиков хранит карточку каждого контрагента"),
        R("Контрагент", "identified-by", "БИН", "Контрагент уникален по БИН", 9),
        R("Контрагент", "references", "Договор поставки", "Карточка контрагента ссылается на договор поставки"),
        R("Филиал", "part-of", "Головное юрлицо", "Филиал входит в головное юрлицо"),
        R("Филиал", "identified-by", "БИН", "имеет свой БИН"),
        # Пересказ, а не слова документа: программа его отвергает.
        R("Головное юрлицо", "identified-by", "БИН", "у головного юрлица всегда один БИН"),
    ]),
    "Каждый пакет документов": ("пакеты документов", [
        E("Пакет документов", "document", "Сканы одного контрагента."),
        E("Контрагенты", "concept"),
        E("Архив сканов", "system"),
        E("Акт сверки", "document"),
    ], [
        R("Пакет документов", "belongs-to", "Контрагенты", "Каждый пакет документов принадлежит одному контрагенту", 8),
        R("Пакет документов", "stored-in", "Архив сканов", "Пакет документов хранится в архиве сканов"),
        R("Акт сверки", "part-of", "Пакет документов", "Акт сверки входит в пакет документов"),
    ]),
    "Сверка с госреестром использует": ("сверка с госреестром", [
        E("Сверка с госреестром", "process", "Проверка карточек по БИН."),
        E("БИН", "attribute"),
        E("Контрагент", "concept"),
        E("Правопреемник", "organization"),
        E("Отчёт о расхождениях", "document"),
    ], [
        R("Сверка с госреестром", "uses", "БИН", "Сверка с госреестром использует БИН карточки"),
        R("Правопреемник", "replaces", "Контрагент", "Ликвидированного контрагента заменяет правопреемник"),
        R("Сверка с госреестром", "produces", "Отчёт о расхождениях", "Сверка порождает отчёт о расхождениях"),
    ]),
    "Правило склейки регулирует": ("правило склейки дублей", [
        E("Правило склейки", "rule", "Как сливаются дубли карточек."),
        E("Реестр поставщиков", "dataset"),
        E("Дубль карточки", "issue"),
        E("БИН", "attribute"),
    ], [
        R("Правило склейки", "governs", "Реестр поставщиков", "Правило склейки регулирует реестр поставщиков", 7),
        R("Дубль карточки", "contradicts", "БИН", "Дубль карточки противоречит уникальности БИН"),
    ]),
    "Сервис payments-api": ("платёжный сервис", [
        E("payments-api", "system", "Платёжный сервис."),
        E("Реестр поставщиков", "dataset"),
        E("Платёжное поручение", "document"),
        E("Договор поставки", "document"),
    ], [
        R("payments-api", "uses", "Реестр поставщиков", "Сервис payments-api использует реестр поставщиков"),
        R("Платёжное поручение", "references", "Договор поставки", "Платёжное поручение ссылается на договор поставки"),
    ]),
    "Ответственный за сверку": ("ответственный за сверку", [
        E("Бухгалтер по расчётам", "role"),
        E("Сверка с госреестром", "process"),
    ], [
        R("Бухгалтер по расчётам", "responsible-for", "Сверка с госреестром",
          "Ответственный за сверку с госреестром — бухгалтер по расчётам"),
    ]),
}

TOOL = r'''#!/usr/bin/python3
# Стоит вместо `claude`: отвечает заготовкой в форме, которую обещает инструмент.
import json, re, sys
answers = json.load(open(%(answers)r, encoding="utf-8"))
prompt = sys.stdin.read()
parts = {int(n): text for n, text in re.findall(r"\[part (\d+)\]\n(.*?)(?=\n\n\[part \d+\]\n|\Z)", prompt, re.S)}
def part_of(quote):
    for number, text in sorted(parts.items()):
        if quote in " ".join(text.split()):
            return number
    return 1
entities, relations, topic = [], [], "заметки"
for marker, (label, known, facts) in answers.items():
    if marker in prompt:
        topic, entities, relations = label, known, facts
        break
else:
    kinds = %(kinds)r
    for subject, verb, obj in re.findall(r"(Модуль \d+) (использует|зависит от|ссылается на|входит в) (модуль \d+)\.", prompt):
        predicate = {"использует": "uses", "зависит от": "depends-on", "ссылается на": "references", "входит в": "part-of"}[verb]
        for name in (subject, obj.capitalize()):
            entities.append({"name": name, "type": kinds[int(name.split()[1]) %% len(kinds)], "description": ""})
        relations.append({"subject": subject, "predicate": predicate, "object": obj.capitalize(), "description": "",
                          "weight": 5, "quote": "%%s %%s %%s" %% (subject, verb, obj)})
for relation in relations:
    relation["part"] = part_of(relation["quote"])
print(json.dumps({"type": "result", "is_error": False, "total_cost_usd": 0.004,
                  "usage": {"input_tokens": len(prompt) // 4, "output_tokens": 300},
                  "structured_output": {"topic": topic, "confidence": 0.8,
                                        "entities": entities, "relations": relations}}, ensure_ascii=False))
'''


def transcript(path, session):
    stamp = lambda minute: "2026-09-21T10:%02d:00.000Z" % minute
    lines = [
        {"type": "custom-title", "sessionId": session, "customTitle": "Кто ведёт сверку"},
        {"type": "user", "sessionId": session, "timestamp": stamp(1),
         "message": {"role": "user", "content": [{"type": "text", "text": "Кто отвечает за сверку?"}]}},
        {"type": "assistant", "sessionId": session, "timestamp": stamp(2),
         "message": {"role": "assistant", "content": [{"type": "text", "text":
                     "Ответственный за сверку с госреестром — бухгалтер по расчётам."}]}},
    ]
    with open(path, "w", encoding="utf-8") as out:
        for line in lines:
            out.write(json.dumps(line, ensure_ascii=False) + "\n")


def scaled(project, size):
    random.seed(48)
    for document in range(max(1, size // 6)):
        sentences = []
        for _ in range(6):
            a, b = random.sample(range(size), 2)
            verb = random.choice(["использует", "зависит от", "ссылается на", "входит в"])
            sentences.append("Модуль %d %s модуль %d." % (a, verb, b))
        with open(os.path.join(project, "module-%04d.md" % document), "w", encoding="utf-8") as out:
            out.write("# Модули, часть %d\n\n%s\n" % (document, "\n".join(sentences)))


def main(root, size):
    root = os.path.realpath(root)
    project = os.path.join(root, "home", "Projects", "supply-registry")
    chats = os.path.join(root, "home", ".claude", "projects", re.sub(r"[^A-Za-z0-9]", "-", project))
    data, tool = os.path.join(root, "data"), os.path.join(root, "tool")
    for directory in (project, chats, data, tool):
        os.makedirs(directory, exist_ok=True)
    if size:
        scaled(project, size)
    else:
        for name, text in DOCUMENTS.items():
            with open(os.path.join(project, name), "w", encoding="utf-8") as out:
                out.write(text)
        transcript(os.path.join(chats, "c3a1-sverka.jsonl"), "c3a1-sverka")

    answers = os.path.join(tool, "answers.json")
    with open(answers, "w", encoding="utf-8") as out:
        json.dump(ANSWERS, out, ensure_ascii=False)
    script = os.path.join(tool, "claude")
    kinds = ["concept", "system", "dataset", "process", "document", "rule"]
    with open(script, "w", encoding="utf-8") as out:
        out.write(TOOL % {"answers": answers, "kinds": kinds})
    os.chmod(script, os.stat(script).st_mode | stat.S_IXUSR)

    config = os.path.join(data, "config.toml")
    with open(config, "w", encoding="utf-8") as out:
        out.write('[analysis]\napproved = ["**/*.jsonl"]\n\n'
                  '[insight]\nmax_run_bytes = 200000000\n\n'
                  '[workspace.demo]\nsend_content_outside = true\ninclude = ["%s", "%s"]\n' % (project, chats))
    environment = dict(os.environ, CROSSKIT_DATA_DIR=data, CROSSKIT_CONFIG=config)
    for directory in (project, chats):
        subprocess.run([CORE, "--context", "demo", "index", "init", directory], env=environment, check=True,
                       stdout=subprocess.DEVNULL)
    run = subprocess.run([CORE, "--context", "demo", "index", "jobs", "run", "--limit", "100000"],
                         env=dict(environment, PATH=tool + ":/usr/bin:/bin"), check=True,
                         capture_output=True, text=True)
    print(run.stdout.strip())
    print("export CROSSKIT_DATA_DIR=%s CROSSKIT_CONFIG=%s" % (data, config))


if __name__ == "__main__":
    arguments = sys.argv[1:]
    size = 0
    if "--scale" in arguments:
        at = arguments.index("--scale")
        size = int(arguments[at + 1])
        del arguments[at:at + 2]
    if len(arguments) != 1:
        sys.exit(__doc__)
    main(arguments[0], size)
