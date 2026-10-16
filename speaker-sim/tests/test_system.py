import json
import unittest
from pathlib import Path
from unittest import mock

from tests.helpers import FakeRunner, temp_paths

import system
from config import ConfigStore

DUMP = [
    {"id": 55, "type": "PipeWire:Interface:Node",
     "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.analog",
                        "node.description": "Audio interne"}}},
    {"id": 60, "type": "PipeWire:Interface:Node",
     "info": {"props": {"media.class": "Audio/Sink", "node.name": "bluez_output.hp"}}},
    {"id": 88, "type": "PipeWire:Interface:Node",
     "info": {"props": {"media.class": "Audio/Sink", "node.name": system.NODE_NAME,
                        "node.description": "Speaker Sim"}}},
    {"id": 70, "type": "PipeWire:Interface:Node",
     "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "brave"}}},
    {"id": 31, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
     "metadata": [
         {"subject": 0, "key": "default.configured.audio.sink", "value": {"name": system.NODE_NAME}},
         {"subject": 0, "key": "default.audio.sink", "value": {"name": "alsa_output.analog"}},
     ]},
]


def dump_runner(dump=DUMP, extra=None):
    responses = {"pw-dump": (0, json.dumps(dump))}
    responses.update(extra or {})
    return FakeRunner(responses)


class PipeWireTests(unittest.TestCase):
    def test_sinks_exclude_simulators(self):
        pw = system.PipeWire(dump_runner())
        self.assertEqual([s.name for s in pw.sinks()], ["alsa_output.analog", "bluez_output.hp"])
        self.assertEqual(len(pw.sinks(include_sim=True)), 3)
        self.assertEqual(pw.sinks()[1].label, "bluez_output.hp")

    def test_default_effective_and_configured(self):
        self.assertEqual(system.parse_default_sink(DUMP), "alsa_output.analog")
        self.assertEqual(system.parse_default_sink(DUMP, configured_only=True), system.NODE_NAME)
        self.assertEqual(system.parse_default_sink([]), "")

    def test_set_default_uses_node_id(self):
        runner = dump_runner()
        system.PipeWire(runner).set_default_sink("bluez_output.hp")
        self.assertIn(["wpctl", "set-default", "60"], runner.calls)

    def test_clock_rate(self):
        runner = FakeRunner({"pw-metadata": (0, "Found \"settings\" metadata 31\n"
                                                "update: id:0 key:'clock.rate' value:'44100' type:''\n")})
        self.assertEqual(system.PipeWire(runner).clock_rate(), 44100)
        self.assertEqual(system.PipeWire(FakeRunner()).clock_rate(), 48000)


class FilterConfigTests(unittest.TestCase):
    def test_graph(self):
        irs = {k: Path(f"/s/ir_{k}.wav") for k in ("LL", "LR", "RL", "RR")}
        conf = system.filter_chain_config(irs, "alsa_output.analog", "Speaker Sim (X)")
        json.dumps(conf)  # sérialisable
        fc = conf["context.modules"][-1]
        self.assertEqual(fc["name"], "libpipewire-module-filter-chain")
        args = fc["args"]
        names = {n["name"] for n in args["filter.graph"]["nodes"]}
        self.assertEqual(names, {"inL", "inR", "convLL", "convLR", "convRL", "convRR", "mixL", "mixR"})
        links = {(l["output"], l["input"]) for l in args["filter.graph"]["links"]}
        self.assertIn(("convRL:Out", "mixL:In 2"), links)   # enceinte droite → oreille gauche
        self.assertIn(("inL:Out", "convLR:In"), links)
        self.assertEqual(args["capture.props"]["node.name"], system.NODE_NAME)
        self.assertEqual(args["playback.props"]["target.object"], "alsa_output.analog")
        conv = next(n for n in args["filter.graph"]["nodes"] if n["name"] == "convLR")
        self.assertEqual(conv["config"]["filename"], "/s/ir_LR.wav")

    def test_no_target(self):
        irs = {k: Path("x") for k in ("LL", "LR", "RL", "RR")}
        conf = system.filter_chain_config(irs, "", "d")
        self.assertNotIn("target.object", conf["context.modules"][-1]["args"]["playback.props"])


class SystemdTests(unittest.TestCase):
    def test_quote(self):
        self.assertEqual(system.exec_line(["/usr/bin/python3", "/a b/main.py", "50%", "$HOME"]),
                         '/usr/bin/python3 "/a b/main.py" "50%%" "$$HOME"')

    def test_unit_file(self):
        tmp, paths = temp_paths()
        with tmp:
            text = system.unit_file_content(paths)
            self.assertIn(f"-c {paths.filter_conf}", text)
            self.assertIn("ExecStartPost=-", text)
            self.assertIn("--hook-stopped", text)
            self.assertIn("WantedBy=default.target", text)

    def test_start_transient_when_not_linked(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (3, ""),
                                 "systemctl --user show": (0, "")})
            system.Systemd(paths, runner).start()
            run = next(c for c in runner.calls if c[0] == "systemd-run")
            self.assertIn(f"--unit={system.UNIT_NAME}", run)
            self.assertEqual(run[-2:], ["-c", str(paths.filter_conf)])
            self.assertTrue(any(a.startswith("ExecStopPost=-") for a in run))

    def test_start_restarts_when_active(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (0, "")})
            system.Systemd(paths, runner).start()
            self.assertIn(["systemctl", "--user", "restart", system.UNIT_NAME], runner.calls)
            self.assertFalse(any(c[0] == "systemd-run" for c in runner.calls))

    def test_enable_autostart_links_unit_from_state_dir(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (3, ""),
                                 "systemctl --user show": (0, "")})
            sd = system.Systemd(paths, runner)
            sd.set_autostart(True)
            self.assertTrue(sd.unit_path.is_file())
            self.assertEqual(sd.unit_path.parent, paths.state_dir)
            self.assertIn(["systemctl", "--user", "link", str(sd.unit_path)], runner.calls)
            self.assertIn(["systemctl", "--user", "enable", system.UNIT_NAME], runner.calls)

    def test_disable_autostart(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-enabled": (0, "linked\n")})
            system.Systemd(paths, runner).set_autostart(False)
            self.assertIn(["systemctl", "--user", "disable", system.UNIT_NAME], runner.calls)


class HookTests(unittest.TestCase):
    def test_started_records_previous_and_switches(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"restarting": True})
            runner = dump_runner()
            system.hook_started(store, system.PipeWire(runner), log=lambda _m: None)
            rt = store.load_runtime()
            self.assertEqual(rt["previous_sink"], "alsa_output.analog")
            self.assertNotIn("restarting", rt)
            self.assertIn(["wpctl", "set-default", "88"], runner.calls)

    def test_started_does_not_record_simulator_as_previous(self):
        tmp, paths = temp_paths()
        with tmp:
            dump = json.loads(json.dumps(DUMP))
            dump[-1]["metadata"][1]["value"] = {"name": "devfrp_speaker_sim_b"}
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp"})
            system.hook_started(store, system.PipeWire(dump_runner(dump)), log=lambda _m: None)
            self.assertEqual(store.load_runtime()["previous_sink"], "bluez_output.hp")

    def test_stopped_restores_previous(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp", "started_at": 1})
            runner = dump_runner()
            system.hook_stopped(store, system.PipeWire(runner), log=lambda _m: None)
            self.assertIn(["wpctl", "set-default", "60"], runner.calls)
            self.assertNotIn("started_at", store.load_runtime())

    def test_stopped_skips_during_restart_or_user_change(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp", "restarting": True})
            runner = dump_runner()
            system.hook_stopped(store, system.PipeWire(runner), log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))

            dump = json.loads(json.dumps(DUMP))
            dump[-1]["metadata"][0]["value"] = {"name": "alsa_output.analog"}  # choix utilisateur
            store.save_runtime({"previous_sink": "bluez_output.hp"})
            runner = dump_runner(dump)
            system.hook_stopped(store, system.PipeWire(runner), log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))

    def test_started_without_node_leaves_default(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            runner = dump_runner(DUMP[:2])
            with mock.patch.object(system.PipeWire, "wait_for_sink", return_value=None):
                system.hook_started(store, system.PipeWire(runner), log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))


if __name__ == "__main__":
    unittest.main()
