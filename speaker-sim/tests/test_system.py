import json
import unittest
from pathlib import Path
from unittest import mock

from tests.helpers import FakeRunner, temp_paths

import system
from config import ConfigStore

NODE_A = system.node_name("a")
NODE_B = system.node_name("b")


def make_dump(configured=NODE_A, effective="alsa_output.analog", sinks_extra=(), links_to=()):
    dump = [
        {"id": 55, "type": "PipeWire:Interface:Node",
         "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.analog",
                            "node.description": "Audio interne"}}},
        {"id": 60, "type": "PipeWire:Interface:Node",
         "info": {"props": {"media.class": "Audio/Sink", "node.name": "bluez_output.hp"}}},
        {"id": 88, "type": "PipeWire:Interface:Node",
         "info": {"props": {"media.class": "Audio/Sink", "node.name": NODE_A,
                            "node.description": "Speaker Sim"}}},
        {"id": 89, "type": "PipeWire:Interface:Node",
         "info": {"props": {"media.class": "Audio/Sink", "node.name": NODE_B,
                            "node.description": "Speaker Sim"}}},
        {"id": 70, "type": "PipeWire:Interface:Node",
         "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "brave"}}},
        {"id": 31, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
         "metadata": [
             {"subject": 0, "key": "default.configured.audio.sink", "value": {"name": configured}},
             {"subject": 0, "key": "default.audio.sink", "value": {"name": effective}},
         ]},
    ]
    dump += list(sinks_extra)
    for i, target_id in enumerate(links_to):
        dump.append({"id": 200 + i, "type": "PipeWire:Interface:Link",
                     "info": {"output-node-id": 70, "input-node-id": target_id}})
    return dump


DUMP = make_dump()


def dump_runner(dump=DUMP, extra=None):
    responses = {"pw-dump": (0, json.dumps(dump))}
    responses.update(extra or {})
    return FakeRunner(responses)


class SlotTests(unittest.TestCase):
    def test_names(self):
        self.assertEqual(system.unit_name("a"), "devfrp-speaker-sim-py-a.service")
        self.assertEqual(system.unit_name(""), system.UNIT_NAME)  # unité v1.0
        self.assertEqual(NODE_B, "devfrp_speaker_sim_py_b")
        self.assertEqual(system.other_slot("a"), "b")
        self.assertTrue(system.is_sim_node(NODE_B))


class PipeWireTests(unittest.TestCase):
    def test_sinks_exclude_simulators(self):
        pw = system.PipeWire(dump_runner())
        self.assertEqual([s.name for s in pw.sinks()], ["alsa_output.analog", "bluez_output.hp"])
        self.assertEqual(len(pw.sinks(include_sim=True)), 4)
        self.assertEqual(pw.sinks()[1].label, "bluez_output.hp")

    def test_default_effective_and_configured(self):
        self.assertEqual(system.parse_default_sink(DUMP), "alsa_output.analog")
        self.assertEqual(system.parse_default_sink(DUMP, configured_only=True), NODE_A)
        self.assertEqual(system.parse_default_sink([]), "")

    def test_set_default_uses_node_id(self):
        runner = dump_runner()
        system.PipeWire(runner).set_default_sink("bluez_output.hp")
        self.assertIn(["wpctl", "set-default", "60"], runner.calls)

    def test_input_links(self):
        pw = system.PipeWire(dump_runner(make_dump(links_to=(88, 88, 89))))
        self.assertEqual(pw.input_links(NODE_A), 2)
        self.assertEqual(pw.input_links(NODE_B), 1)
        self.assertEqual(pw.input_links("absent"), 0)
        self.assertTrue(system.PipeWire(dump_runner()).wait_streams_left(NODE_A, timeout=0.5))
        self.assertFalse(pw.wait_streams_left(NODE_A, timeout=0.3))

    def test_clock_rate(self):
        runner = FakeRunner({"pw-metadata": (0, "Found \"settings\" metadata 31\n"
                                                "update: id:0 key:'clock.rate' value:'44100' type:''\n")})
        self.assertEqual(system.PipeWire(runner).clock_rate(), 44100)
        self.assertEqual(system.PipeWire(FakeRunner()).clock_rate(), 48000)


class FilterConfigTests(unittest.TestCase):
    def test_graph(self):
        irs = {k: Path(f"/s/ir_{k}.wav") for k in ("LL", "LR", "RL", "RR")}
        conf = system.filter_chain_config(irs, "alsa_output.analog", "Speaker Sim (X)", "b")
        json.dumps(conf)
        args = conf["context.modules"][-1]["args"]
        names = {n["name"] for n in args["filter.graph"]["nodes"]}
        self.assertEqual(names, {"inL", "inR", "convLL", "convLR", "convRL", "convRR", "mixL", "mixR"})
        links = {(l["output"], l["input"]) for l in args["filter.graph"]["links"]}
        self.assertIn(("convRL:Out", "mixL:In 2"), links)   # enceinte droite → oreille gauche
        self.assertIn(("inL:Out", "convLR:In"), links)
        self.assertEqual(args["capture.props"]["node.name"], NODE_B)
        self.assertEqual(args["playback.props"]["node.name"], NODE_B + ".output")
        self.assertEqual(args["playback.props"]["target.object"], "alsa_output.analog")

    def test_no_target(self):
        irs = {k: Path("x") for k in ("LL", "LR", "RL", "RR")}
        conf = system.filter_chain_config(irs, "", "d")
        self.assertNotIn("target.object", conf["context.modules"][-1]["args"]["playback.props"])

    def test_write_per_slot(self):
        tmp, paths = temp_paths()
        with tmp:
            irs = {k: Path("x") for k in ("LL", "LR", "RL", "RR")}
            path = system.write_filter_config(paths, irs, "", "d", "b")
            self.assertEqual(path, paths.filters_dir / "b" / "filter-chain.conf")


class SystemdTests(unittest.TestCase):
    def test_quote(self):
        self.assertEqual(system.exec_line(["/usr/bin/python3", "/a b/main.py", "50%", "$HOME"]),
                         '/usr/bin/python3 "/a b/main.py" "50%%" "$$HOME"')

    def test_unit_file(self):
        tmp, paths = temp_paths()
        with tmp:
            text = system.unit_file_content(paths, "a")
            self.assertIn(f"-c {paths.slot_conf('a')}", text)
            self.assertIn("--hook-started a", text)
            self.assertIn("--hook-stopped a", text)
            self.assertIn("WantedBy=default.target", text)

    def test_start_transient_when_not_linked(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (3, ""),
                                 "systemctl --user show": (0, "")})
            system.Systemd(paths, runner).start("b")
            run = next(c for c in runner.calls if c[0] == "systemd-run")
            self.assertIn("--unit=devfrp-speaker-sim-py-b.service", run)
            self.assertEqual(run[-2:], ["-c", str(paths.slot_conf("b"))])
            self.assertTrue(any(a.endswith("--hook-stopped b") for a in run))

    def test_active_slots(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active --quiet devfrp-speaker-sim-py-b": (0, ""),
                                 "systemctl --user is-active": (3, "")})
            sd = system.Systemd(paths, runner)
            self.assertEqual(sd.active_slots(), ["b"])
            self.assertTrue(sd.is_active())
            self.assertFalse(sd.legacy_active())

    def test_stop_all_includes_legacy(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (0, "")})
            system.Systemd(paths, runner).stop()
            stopped = [c[-1] for c in runner.calls if c[:3] == ["systemctl", "--user", "stop"]]
            self.assertEqual(stopped, ["devfrp-speaker-sim-py-a.service",
                                       "devfrp-speaker-sim-py-b.service", system.UNIT_NAME])

    def test_enable_autostart_links_slot_a(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (3, ""),
                                 "systemctl --user show": (0, "")})
            sd = system.Systemd(paths, runner)
            sd.set_autostart(True)
            self.assertTrue(sd.unit_path("a").is_file())
            self.assertEqual(sd.unit_path("a").parent, paths.state_dir)
            self.assertIn(["systemctl", "--user", "link", str(sd.unit_path("a"))], runner.calls)
            self.assertIn(["systemctl", "--user", "enable", "devfrp-speaker-sim-py-a.service"],
                          runner.calls)

    def test_enable_refused_while_transient_a_runs(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-active": (0, ""),
                                 "systemctl --user show": (0, "")})
            with self.assertRaises(system.SystemError_):
                system.Systemd(paths, runner).set_autostart(True)

    def test_disable_autostart(self):
        tmp, paths = temp_paths()
        with tmp:
            runner = FakeRunner({"systemctl --user is-enabled": (0, "linked\n")})
            system.Systemd(paths, runner).set_autostart(False)
            self.assertIn(["systemctl", "--user", "disable", "devfrp-speaker-sim-py-a.service"],
                          runner.calls)


class HookTests(unittest.TestCase):
    def test_started_records_previous_and_switches(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"restarting": True})
            runner = dump_runner(make_dump(configured="alsa_output.analog"))
            system.hook_started(store, system.PipeWire(runner), "a", log=lambda _m: None)
            rt = store.load_runtime()
            self.assertEqual(rt["previous_sink"], "alsa_output.analog")
            self.assertEqual(rt["active_slot"], "a")
            self.assertNotIn("restarting", rt)
            self.assertIn(["wpctl", "set-default", "88"], runner.calls)

    def test_switch_keeps_previous_sink(self):
        """B démarre alors que A est la sortie : la sortie « casque » mémorisée reste."""
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp", "active_slot": "a"})
            runner = dump_runner(make_dump(configured=NODE_A, effective=NODE_A))
            system.hook_started(store, system.PipeWire(runner), "b", log=lambda _m: None)
            rt = store.load_runtime()
            self.assertEqual(rt["previous_sink"], "bluez_output.hp")
            self.assertEqual(rt["active_slot"], "b")
            self.assertIn(["wpctl", "set-default", "89"], runner.calls)

    def test_switch_old_instance_stop_does_not_restore(self):
        """A s'arrête après la bascule : la sortie choisie est B, on ne touche à rien."""
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp", "active_slot": "b"})
            runner = dump_runner(make_dump(configured=NODE_B, effective=NODE_B))
            system.hook_stopped(store, system.PipeWire(runner), "a", log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))
            self.assertEqual(store.load_runtime()["active_slot"], "b")

    def test_stopped_restores_previous(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp", "started_at": 1,
                                "active_slot": "a"})
            runner = dump_runner()
            system.hook_stopped(store, system.PipeWire(runner), "a", log=lambda _m: None)
            self.assertIn(["wpctl", "set-default", "60"], runner.calls)
            rt = store.load_runtime()
            self.assertNotIn("started_at", rt)
            self.assertNotIn("active_slot", rt)

    def test_stopped_skips_when_user_chose_other_output(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp"})
            runner = dump_runner(make_dump(configured="alsa_output.analog"))
            system.hook_stopped(store, system.PipeWire(runner), "a", log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))

    def test_legacy_hook(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            store.save_runtime({"previous_sink": "bluez_output.hp"})
            dump = make_dump(configured=system.NODE_NAME)
            runner = dump_runner(dump)
            system.hook_stopped(store, system.PipeWire(runner), "", log=lambda _m: None)
            self.assertIn(["wpctl", "set-default", "60"], runner.calls)

    def test_started_without_node_leaves_default(self):
        tmp, paths = temp_paths()
        with tmp:
            store = ConfigStore(paths)
            runner = dump_runner(DUMP[:2])
            with mock.patch.object(system.PipeWire, "wait_for_sink", return_value=None):
                system.hook_started(store, system.PipeWire(runner), "a", log=lambda _m: None)
            self.assertFalse(any(c[0] == "wpctl" for c in runner.calls))


if __name__ == "__main__":
    unittest.main()
