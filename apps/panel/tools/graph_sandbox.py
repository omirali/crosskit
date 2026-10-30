#!/usr/bin/env python3
"""Выдуманный контекст для снимков графа панели (этап UI-4b).

Снимки в apps/panel/docs/ снимаются не с дерева владельца: в них не должно быть ни одного
настоящего имени, пути или беседы. Этот скрипт строит маленький контекст `demo` целиком из
выдуманного — проект `payments-api`, папку его бесед в том виде, в каком её ведёт Claude
Code, и пять бесед, каждая под одно состояние экрана:

* «Договоры в payments-api» — 2 100 реплик и сорок файлов, один из них записан относительным
  путём, один вне контекста, один удалён с диска: большая беседа;
* «Сверка договоров», «Ретраи webhook», «Отчёт по договорам» — трогают тот же `contract.rs`:
  файл, который трогали многие беседы;
* «Вопрос без касаний» — ни одного файлового вызова;
* «Разбор реестра» — правит файл, которого больше никто не трогал, и путь вне контекста:
  пустой второй шаг и файл «вне контекста».

Запуск (из корня репозитория, ядро собрано `cargo build --release`):

    python3 apps/panel/tools/graph_sandbox.py <каталог>

Печатает переменные окружения, с которыми ядро и панель видят этот контекст, и фокусы для
`--graph-focus`.
"""
import json
import os
import re
import subprocess
import sys

CORE = os.path.abspath("target/release/crosskit")


def encoded(directory):
    """Имя папки бесед, которое Claude Code даёт каталогу (src/domain/transcript_folder.rs)."""
    return re.sub(r"[^A-Za-z0-9]", "-", directory)


def stamp(minute):
    return "2026-09-%02dT%02d:%02d:00.000Z" % (3 + minute // 1440, (minute // 60) % 24, minute % 60)


class Transcript:
    def __init__(self, session, title):
        self.session = session
        self.lines = [{"type": "custom-title", "sessionId": session, "customTitle": title}]
        self.minute = 0

    def say(self, role, text):
        self.minute += 1
        self.lines.append({"type": role, "sessionId": self.session, "timestamp": stamp(self.minute),
                           "message": {"role": role, "content": [{"type": "text", "text": text}]}})

    def tool(self, name, path):
        self.lines.append({"type": "assistant", "sessionId": self.session, "timestamp": stamp(self.minute),
                           "message": {"role": "assistant",
                                       "content": [{"type": "tool_use", "name": name,
                                                    "input": {"file_path": path}}]}})

    def write(self, folder):
        with open(os.path.join(folder, self.session + ".jsonl"), "w", encoding="utf-8") as out:
            for line in self.lines:
                out.write(json.dumps(line, ensure_ascii=False) + "\n")


def main(root):
    root = os.path.realpath(root)
    project = os.path.join(root, "home", "Projects", "payments-api")
    chats = os.path.join(root, "home", ".claude", "projects", encoded(project))
    data = os.path.join(root, "data")
    for directory in (project, chats, data):
        os.makedirs(directory, exist_ok=True)

    names = ["contract.rs", "settlement.rs", "registry.rs", "retry.ts", "webhook.ts", "ledger.rs",
             "invoice.rs", "payout.rs", "refund.rs", "currency.rs", "tax.rs", "audit.rs",
             "schema.sql", "migrate_001.sql", "migrate_002.sql", "README.md", "draft.md",
             "notes.md", "config.toml", "routes.ts"]
    files = []
    for index in range(40):
        name = names[index % len(names)]
        # Во второй половине — свои имена: два `ledger.rs` на снимке читались бы как дубль.
        sub, name = ("src", name) if index < 20 else ("lib", "lib_" + name)
        path = os.path.join(project, sub, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as out:
            out.write("// выдуманный файл %d\n" % index)
        files.append(path)
    contract = files[0]
    registry_only = os.path.join(project, "src", "registry_import.rs")
    with open(registry_only, "w", encoding="utf-8") as out:
        out.write("// импорт реестра\n")
    gone = os.path.join(project, "notes", "draft-contract.md")
    os.makedirs(os.path.dirname(gone), exist_ok=True)
    with open(gone, "w", encoding="utf-8") as out:
        out.write("черновик, который потом удалят\n")

    big = Transcript("5e55-10n-big", "Договоры в payments-api")
    for turn in range(1050):
        big.say("user", "шаг %d по договорам" % turn)
        big.say("assistant", "делаю шаг %d" % turn)
        if turn < len(files):
            big.tool("Edit" if turn % 3 else "Write", files[turn])
            if turn % 5 == 0:
                big.say("user", "посмотри %s" % os.path.basename(files[turn]))
    big.tool("Edit", "src/settlement.rs")
    big.tool("Edit", "/elsewhere/shared/registry.rs")
    big.tool("Write", gone)
    big.write(chats)

    for session, title, extra in (("7c1d-sverka", "Сверка договоров", files[1]),
                                  ("8a2e-retry", "Ретраи webhook", files[3]),
                                  ("9b3f-report", "Отчёт по договорам", files[5])):
        other = Transcript(session, title)
        other.say("user", "открой contract.rs")
        other.say("assistant", "открываю")
        other.tool("Edit", contract)
        other.tool("Edit", extra)
        other.write(chats)

    quiet = Transcript("a4c0-quiet", "Вопрос без касаний")
    quiet.say("user", "как устроены договоры в целом?")
    quiet.say("assistant", "договоры хранятся в реестре и сверяются раз в сутки")
    quiet.write(chats)

    alone = Transcript("b5d1-alone", "Разбор реестра")
    alone.say("user", "поправь registry_import.rs")
    alone.say("assistant", "правлю")
    alone.tool("Edit", registry_only)
    alone.tool("Edit", "/elsewhere/shared/registry.rs")
    alone.write(chats)

    config = os.path.join(data, "config.toml")
    with open(config, "w", encoding="utf-8") as out:
        out.write('[workspace.demo]\ninclude = ["%s", "%s"]\n' % (project, chats))
    environment = dict(os.environ, CROSSKIT_DATA_DIR=data, CROSSKIT_CONFIG=config)
    for directory in (project, chats):
        subprocess.run([CORE, "--context", "demo", "index", "init", directory], env=environment, check=True,
                       stdout=subprocess.DEVNULL)
    os.remove(gone)
    subprocess.run([CORE, "--context", "demo", "index", "reconcile", project], env=environment, check=True,
                   stdout=subprocess.DEVNULL)

    print("export CROSSKIT_DATA_DIR=%s CROSSKIT_CONFIG=%s" % (data, config))
    print("conversation:5e55-10n-big     большая беседа")
    print("path:%s     файл многих бесед" % contract)
    print("conversation:a4c0-quiet       беседа без касаний")
    print("conversation:b5d1-alone       пустой второй шаг")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
