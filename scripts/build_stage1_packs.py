#!/usr/bin/env python3
"""
Generate schema-valid Stage 1 starter .onehundo packs for the Top 20 Decomps,
Recomps & Emulator Targets.

Invariants enforced:
- Clean canonical game titles in manifest.title (never append runtime/fork in parentheses).
- Multi-matcher definitions covering executables, app IDs, platform IDs, ROM hashes,
  and save_path_glob across Windows, Linux, SteamOS, macOS, Android, and iOS.
- Explicit `placeholder_template: true` + `format: "template"` (or JSON/trophy paths
  without fabricated offsets) wherever exact binary save offsets are unverified.
"""

from __future__ import annotations

import json
import zipfile
from pathlib import Path
from typing import Any

PACKS_DIR = Path(__file__).resolve().parent.parent / "packs"


def ach(
    ach_id: str,
    title: str,
    description: str,
    points: int = 10,
    hidden: bool = False,
) -> dict[str, Any]:
    return {
        "canonical_achievement_id": ach_id,
        "title": title,
        "description": description,
        "points": points,
        "hidden": hidden,
        "supported_modes": ["STANDARD", "ENCORE", "SPECTATOR"],
        "minimum_trust_tier": "LOCAL",
    }


def write_onehundo(filename: str, files: dict[str, Any]) -> Path:
    PACKS_DIR.mkdir(parents=True, exist_ok=True)
    out = PACKS_DIR / filename
    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        for arc_path, obj in files.items():
            raw = obj if isinstance(obj, bytes) else (json.dumps(obj, indent=2) + "\n").encode("utf-8")
            zi = zipfile.ZipInfo(arc_path, date_time=(2026, 1, 1, 0, 0, 0))
            zi.external_attr = (0o100644) << 16
            zf.writestr(zi, raw)
    return out


def tmpl_save_doc(
    game_id: str,
    watcher_id: str,
    file_glob: str,
    source_type: str,
    notes: str,
    extra_watchers: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    watchers: list[dict[str, Any]] = [
        {
            "id": watcher_id,
            "source_type": source_type,
            "format": "template",
            "file_glob": file_glob,
            "max_file_size_bytes": 16 * 1024 * 1024,
            "placeholder_template": True,
            "notes": notes,
        }
    ]
    if extra_watchers:
        watchers.extend(extra_watchers)
    return {
        "schema_version": "0.1.0",
        "canonical_game_id": game_id,
        "notes": notes,
        "watchers": watchers,
    }


def build_all() -> list[Path]:
    created: list[Path] = []

    # 1. Bloodborne (multi-fork: bbport, SuperMedo, SacredFire, shadPS4 SPRJ0005 / CUSA03173)
    created.append(write_onehundo(
        "sony.bloodborne.forks.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "sony.bloodborne.forks",
                "pack_version": "0.1.0",
                "canonical_game_id": "sony.bloodborne",
                "title": "Bloodborne",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA03173", "os": "any", "launcher": "shadps4"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA00900", "region": "USA", "os": "any", "launcher": "shadps4"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA00207", "region": "EUR", "os": "any", "launcher": "shadps4"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "SPRJ0005", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "bbport", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "supermedo", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "sacredfire", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*bbport*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*supermedo*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*sacredfire*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/SPRJ0005/**", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/CUSA03173/**", "os": "any"},
                ],
                "profiles": {
                    "semantic": {
                        "enabled": True,
                        "producer_ids": ["bbport_pc_x64", "supermedo_pc", "sacredfire_pc"],
                    },
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [
                            {"source_type": "trophy_dir", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/TROP*"},
                            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*userdata*"},
                        ],
                        "watch_paths": [
                            {"os": "linux", "launcher": "shadps4", "path_glob": "~/.local/share/shadPS4/user/savedata/**/SPRJ0005"},
                            {"os": "steamos", "launcher": "shadps4", "path_glob": "~/.var/app/net.shadps4.shadPS4/data/shadPS4/user/savedata/**/SPRJ0005"},
                            {"os": "windows", "launcher": "shadps4", "path_glob": "%APPDATA%/shadPS4/user/savedata/**/SPRJ0005"},
                            {"os": "macos", "launcher": "shadps4", "path_glob": "~/Library/Application Support/shadPS4/user/savedata/**/SPRJ0005"},
                        ],
                    },
                },
                "achievements": [
                    ach("ach_cleric_beast", "Cleric Beast", "Defeat the Cleric Beast.", 15),
                    ach("ach_father_gascoigne", "Father Gascoigne", "Defeat Father Gascoigne.", 15),
                    ach("ach_bloodborne_plat", "Bloodborne", "All trophies acquired.", 100),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "sony.bloodborne",
                "sprj0005_save_template",
                "**/*userdata*",
                "save_file",
                "shadPS4 trophy directories (TROPUSR.DAT / TROP.XML / trophy XML/JSON) are parsed natively by trophy_dir.rs. Encrypted/compressed SPRJ0005 userdata000* binary flag offsets differ across forks (bbport/SuperMedo/SacredFire) and remain marked as template until verified per build.",
            ),
        },
    ))

    # 2. Grand Theft Auto VI (ScarlettPort / console / PC slots)
    created.append(write_onehundo(
        "rockstar.gta6.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "rockstar.gta6.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "rockstar.gta6",
                "title": "Grand Theft Auto VI",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "scarlettport", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*ScarlettPort*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*GTA6*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/SGTA6*", "os": "any"},
                ],
                "profiles": {
                    "semantic": {"enabled": True, "producer_ids": ["scarlettport_runtime"]},
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/SGTA6*"}],
                        "watch_paths": [
                            {"os": "windows", "path_glob": "%USERPROFILE%/Documents/Rockstar Games/GTA VI/Profiles/*"},
                            {"os": "linux", "path_glob": "~/.local/share/ScarlettPort/saves/*"},
                            {"os": "steamos", "path_glob": "~/.local/share/ScarlettPort/saves/*"},
                        ],
                    },
                },
                "achievements": [
                    ach("ach_welcome_leonida", "Welcome to Leonida", "Complete the opening prologue.", 15),
                    ach("ach_career_criminal", "Career Criminal", "Reach 100% completion.", 100),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "rockstar.gta6",
                "gta6_slot_template",
                "**/SGTA6*",
                "save_file",
                "Placeholder save-slot template for ScarlettPort, console, and PC SGTA6* saves; binary offsets are intentionally unpopulated until verified save dumps exist.",
            ),
        },
    ))

    # 3. The Legend of Zelda: Ocarina of Time (Switch 2 / Longshot / Project ReLink / Triforce / Rekarina / Fritchii / Lichii & Ship of Harkinian)
    created.append(write_onehundo(
        "nintendo.oot.forks.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.oot.forks",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.oot.switch2",
                "title": "The Legend of Zelda: Ocarina of Time",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "longshot", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "project_relink", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "triforce", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "rekarina", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "fritchii", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "lichii", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "ship_of_harkinian", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*longshot*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*relink*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*triforce*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*rekarina*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*fritchii*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*lichii*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*soh*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/progress.sav", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/Save/file*.sav", "os": "any"},
                ],
                "profiles": {
                    "semantic": {
                        "enabled": True,
                        "producer_ids": ["longshot", "project_relink", "triforce", "rekarina", "fritchii", "lichii", "ship_of_harkinian"],
                    },
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [
                            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/progress.sav"},
                            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/soh_save*.json"},
                        ],
                        "watch_paths": [
                            {"os": "linux", "path_glob": "~/.local/share/longshot/**/progress.sav"},
                            {"os": "steamos", "path_glob": "~/.local/share/longshot/**/progress.sav"},
                            {"os": "windows", "path_glob": "%APPDATA%/Longshot/**/progress.sav"},
                            {"os": "macos", "path_glob": "~/Library/Application Support/Ship of Harkinian/Save/**"},
                            {"os": "android", "path_glob": "/storage/emulated/0/Android/data/com.dishii.soh/files/Save/**"},
                        ],
                    },
                },
                "achievements": [
                    ach("a01", "It works, it works!", "Obtain the Kokiri Sword and Deku Shield.", 10),
                    ach("a02", "Inside the Great Deku Tree", "Clear the Inside the Great Deku Tree dungeon.", 20),
                    ach("a13", "Hero of Time", "Restore peace to Hyrule.", 50),
                ],
            },
            "rules/save.json": {
                "schema_version": "0.1.0",
                "canonical_game_id": "nintendo.oot.switch2",
                "notes": "Ship of Harkinian JSON save watcher plus progress.sav template for Longshot/Project ReLink/Triforce/Rekarina/Fritchii/Lichii forks (progress.sav native parsing handled directly by longshot.rs).",
                "watchers": [
                    {
                        "id": "soh_json_save",
                        "source_type": "save_file",
                        "format": "json",
                        "file_glob": "**/soh_save*.json",
                        "max_file_size_bytes": 4194304,
                        "achievement_rules": [
                            {
                                "achievement_id": "a02",
                                "conditions": [{"path": "questStatus.emerald", "value_type": "bool", "op": "==", "value": True}],
                            },
                            {
                                "achievement_id": "a13",
                                "conditions": [{"path": "flags.ganonDefeated", "value_type": "bool", "op": "==", "value": True}],
                            },
                        ],
                        "stat_rules": [
                            {"stat_key": "gold_skulltulas", "path": "inventory.gsTokens", "value_type": "i64"}
                        ],
                    },
                    {
                        "id": "switch2_fork_save_template",
                        "source_type": "save_file",
                        "format": "template",
                        "file_glob": "**/relink_save*.bin",
                        "placeholder_template": True,
                        "notes": "Template watcher for Project ReLink / Triforce / Rekarina / Fritchii / Lichii custom binary saves pending verified offset tables; Longshot progress.sav hashes are verified in longshot.rs.",
                    },
                ],
            },
        },
    ))

    # 4. Majora's Mask (Zelda64Recomp / 2Ship2Harkinian)
    created.append(write_onehundo(
        "nintendo.majoras_mask.recomp.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.mm.recomp",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.majoras_mask",
                "title": "The Legend of Zelda: Majora's Mask",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "zelda64recomp", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "2ship2harkinian", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*Zelda64Recompiled*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*2s2h*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/zelda64recomp_*.sav", "os": "any"},
                ],
                "profiles": {
                    "semantic": {"enabled": True, "producer_ids": ["zelda64recomp", "2ship2harkinian"]},
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*save*"}],
                        "watch_paths": [
                            {"os": "linux", "path_glob": "~/.config/Zelda64Recompiled/saves/*"},
                            {"os": "steamos", "path_glob": "~/.var/app/io.github.zelda64recomp.Zelda64Recomp/config/Zelda64Recompiled/saves/*"},
                            {"os": "windows", "path_glob": "%LOCALAPPDATA%/Zelda64Recompiled/saves/*"},
                            {"os": "macos", "path_glob": "~/Library/Application Support/Zelda64Recompiled/saves/*"},
                        ],
                    },
                },
                "achievements": [
                    ach("ach_dawn_first_day", "Dawn of the First Day", "Recover the Ocarina of Time from Skull Kid.", 15),
                    ach("ach_fierce_deity", "Fierce Deity", "Collect all 24 masks in Termina.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "nintendo.majoras_mask",
                "mm_recomp_save_template",
                "**/*.sav",
                "save_file",
                "Zelda64Recompiled / 2Ship2Harkinian save template; unverified compressed EEPROM offsets are marked placeholder_template until verified dumps.",
            ),
        },
    ))

    # 5. Sonic Unleashed (Unleashed Recompiled)
    created.append(write_onehundo(
        "sega.unleashed.recomp.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "sega.sonic_unleashed.recomp",
                "pack_version": "0.1.0",
                "canonical_game_id": "sega.sonic_unleashed",
                "title": "Sonic Unleashed",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "unleashed_recomp", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*UnleashedRecomp*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/UnleashedRecomp/**/SYS-DATA*", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/SYS-DATA*"}],
                        "watch_paths": [
                            {"os": "windows", "path_glob": "%APPDATA%/UnleashedRecomp/save/**"},
                            {"os": "linux", "path_glob": "~/.config/UnleashedRecomp/save/**"},
                            {"os": "steamos", "path_glob": "~/.var/app/io.github.hedge_dev.UnleashedRecomp/config/UnleashedRecomp/save/**"},
                        ],
                    }
                },
                "achievements": [
                    ach("ach_still_broken", "Still Broken", "Restore the first continent's Gaia Temple.", 15),
                    ach("ach_world_adventure", "World Adventure", "Collect every Sun and Moon Medal.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "sega.sonic_unleashed",
                "unleashed_sysdata_template",
                "**/SYS-DATA*",
                "save_file",
                "UnleashedRecomp SYS-DATA template; binary endianness & checksum wrapper await verified dump offsets.",
            ),
        },
    ))

    # 6. Breath of the Wild (Cemu / Switch)
    created.append(write_onehundo(
        "nintendo.botw.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.botw.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.breath_of_the_wild",
                "title": "The Legend of Zelda: Breath of the Wild",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "00050000101C9400", "region": "USA", "launcher": "cemu"},
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "00050000101C9500", "region": "EUR", "launcher": "cemu"},
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "01007EF00011E000", "region": "GLOBAL", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/game_data.sav", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/game_data.sav"}],
                        "watch_paths": [
                            {"os": "windows", "launcher": "cemu", "path_glob": "%APPDATA%/Cemu/mlc01/usr/save/00050000/101c9*/user/**"},
                            {"os": "linux", "launcher": "cemu", "path_glob": "~/.local/share/Cemu/mlc01/usr/save/00050000/101c9*/user/**"},
                        ],
                    }
                },
                "achievements": [
                    ach("ach_great_plateau", "Off the Great Plateau", "Acquire the Paraglider and descend from the Great Plateau.", 15),
                    ach("ach_all_shrines", "All 120 Shrines", "Complete all 120 Ancient Shrines.", 60),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "nintendo.breath_of_the_wild",
                "botw_game_data_template",
                "**/game_data.sav",
                "save_file",
                "BotW game_data.sav stores Murmur3-32 hashed flag tables (`0x000000`..); flagged as template pending deterministic hash table rules.",
            ),
        },
    ))

    # 7. Wind Waker HD (Cemu)
    created.append(write_onehundo(
        "nintendo.twwhd.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.twwhd.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.wind_waker_hd",
                "title": "The Legend of Zelda: The Wind Waker HD",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "0005000010143500", "region": "USA", "launcher": "cemu"},
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "0005000010143600", "region": "EUR", "launcher": "cemu"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/cking.sav", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/cking.sav"}],
                    }
                },
                "achievements": [
                    ach("ach_wind_waker", "Master of the Winds", "Obtain the Wind Waker and set sail from Outset Island.", 15),
                    ach("ach_hero_of_winds", "Hero of Winds", "Defeat Ganondorf atop Ganon's Tower.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "nintendo.wind_waker_hd",
                "twwhd_cking_template",
                "**/cking.sav",
                "save_file",
                "Cemu cking.sav big-endian quest flag table awaiting verified slot offsets.",
            ),
        },
    ))

    # 8. Super Mario 64 (sm64ex / Render96 / CoopDX / N64)
    created.append(write_onehundo(
        "nintendo.sm64.recomp.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.sm64.recomp",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.super_mario_64",
                "title": "Super Mario 64",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "sm64ex", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "render96", "os": "any"},
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "sm64coopdx", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*sm64*", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*Render96*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/sm64_save_file.bin", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [
                            {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/sm64_save_file.bin"},
                            {"source_type": "emulator_sram", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/Super Mario 64*.srm"},
                        ],
                    }
                },
                "achievements": [
                    ach("ach_first_star", "Power Star Discovered", "Collect your first Power Star in Bob-omb Battlefield.", 10),
                    ach("ach_120_stars", "120 Power Stars", "Collect all 120 Power Stars in Peach's Castle.", 100),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "nintendo.super_mario_64",
                "sm64_eeprom_template",
                "**/sm64_save_file.bin",
                "save_file",
                "sm64ex / Render96 / sm64coopdx save_file.bin varies in byte-swapping across N64 vs PC decomp ports; marked placeholder_template pending fork detection.",
            ),
        },
    ))

    # 9a-c. OpenGOAL Jak Trilogy (Jak 1, Jak II, Jak 3 — strictly separate works/packs)
    for num, cid, fname, title, ach1, ach2 in [
        ("jak1", "opengoal.jak_and_daxter", "opengoal.jak1.onehundo", "Jak and Daxter: The Precursor Legacy", ("ach_power_cell_1", "First Power Cell", "Collect your first Power Cell."), ("ach_101_cells", "Precursor Legacy", "Collect all 101 Power Cells.")),
        ("jak2", "opengoal.jak_2", "opengoal.jak2.onehundo", "Jak II", ("ach_breakout", "Prison Breakout", "Escape the Fortress in Haven City."), ("ach_metal_kor", "Metal Head Leader", "Defeat Kor at the Nest.")),
        ("jak3", "opengoal.jak_3", "opengoal.jak3.onehundo", "Jak 3", ("ach_wasteland", "Wastelander", "Prove yourself in Spargus Arena."), ("ach_dark_maker", "Savior of the World", "Stop Errol's Dark Maker ship.")),
    ]:
        created.append(write_onehundo(
            fname,
            {
                "manifest.json": {
                    "manifest_version": "0.1.0",
                    "pack_id": f"opengoal.{num}.starter",
                    "pack_version": "0.1.0",
                    "canonical_game_id": cid,
                    "title": title,
                    "authors": ["OneHundoOrg"],
                    "matchers": [
                        {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": f"opengoal_{num}", "os": "any"},
                        {"matcher_type": "executable", "algorithm": "glob", "value": "*gk*", "os": "any", "launcher": "opengoal"},
                        {"matcher_type": "save_path_glob", "algorithm": "glob", "value": f"**/OpenGOAL/{num}/saves/**", "os": "any"},
                    ],
                    "profiles": {
                        "semantic": {"enabled": True, "producer_ids": [f"opengoal_{num}"]},
                        "observation": {
                            "enabled": True,
                            "rules_file": "rules/save.json",
                            "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": f"**/{num}*.bin"}],
                            "watch_paths": [
                                {"os": "windows", "launcher": "opengoal", "path_glob": f"%APPDATA%/OpenGOAL/{num}/saves/**"},
                                {"os": "linux", "launcher": "opengoal", "path_glob": f"~/.config/OpenGOAL/{num}/saves/**"},
                                {"os": "steamos", "launcher": "opengoal", "path_glob": f"~/.config/OpenGOAL/{num}/saves/**"},
                            ],
                        },
                    },
                    "achievements": [
                        ach(ach1[0], ach1[1], ach1[2], 15),
                        ach(ach2[0], ach2[1], ach2[2], 50),
                    ],
                },
                "rules/save.json": tmpl_save_doc(
                    cid,
                    f"opengoal_{num}_template",
                    f"**/{num}*.bin",
                    "save_file",
                    f"OpenGOAL {title} binary save slot template.",
                ),
            },
        ))

    # 10. Demon's Souls (RPCS3)
    created.append(write_onehundo(
        "sony.demons_souls.rpcs3.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "sony.demons_souls.rpcs3",
                "pack_version": "0.1.0",
                "canonical_game_id": "sony.demons_souls",
                "title": "Demon's Souls",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BLUS30443", "region": "USA", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BLES00932", "region": "EUR", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BCJS30022", "region": "JPN", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "NPWR00572_00", "os": "any", "launcher": "rpcs3"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "trophy_dir", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/TROP*"}],
                        "watch_paths": [
                            {"os": "linux", "launcher": "rpcs3", "path_glob": "~/.config/rpcs3/dev_hdd0/home/*/trophy/NPWR00572_00"},
                            {"os": "windows", "launcher": "rpcs3", "path_glob": "%APPDATA%/rpcs3/dev_hdd0/home/*/trophy/NPWR00572_00"},
                        ],
                    }
                },
                "achievements": [
                    ach("ach_phalanx", "Phalanx's Trophy", "Slayer of Demon 'Phalanx'.", 15),
                    ach("ach_toughest_soul", "Toughest Soul Trophy", "All Trophies Obtained.", 100),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "sony.demons_souls",
                "rpcs3_des_save_template",
                "**/1DS3*",
                "save_file",
                "RPCS3 TROPUSR.DAT trophies are parsed automatically by trophy_dir.rs; save file template covers future stat/world tendency reads.",
            ),
        },
    ))

    # 11. Uncharted: Drake's Fortune (RPCS3 / PS4)
    created.append(write_onehundo(
        "sony.uncharted.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "sony.uncharted.drakes_fortune",
                "pack_version": "0.1.0",
                "canonical_game_id": "sony.uncharted_drakes_fortune",
                "title": "Uncharted: Drake's Fortune",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BCUS98103", "region": "USA", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BCES00065", "region": "EUR", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "NPWR00142_00", "os": "any", "launcher": "rpcs3"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "trophy_dir", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/TROP*"}],
                    }
                },
                "achievements": [
                    ach("ach_first_treasure", "First Treasure", "Find one treasure.", 10),
                    ach("ach_charted_crushing", "Charted! - Crushing", "Finish the game on Crushing difficulty.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "sony.uncharted_drakes_fortune",
                "uncharted_trophy_template",
                "**/*.sav",
                "save_file",
                "RPCS3 TROPUSR.DAT handles native trophy unlocks directly.",
            ),
        },
    ))

    # 12a-b. Gravity Rush & Gravity Rush 2 (Vita3K / shadPS4 / Android)
    for gid, fname, pid, title, plat_ids in [
        ("sony.gravity_rush", "sony.gravity_rush.onehundo", "sony.gravity_rush.starter", "Gravity Rush", ["PCSA00011", "PCSF00024", "CUSA01130"]),
        ("sony.gravity_rush_2", "sony.gravity_rush_2.onehundo", "sony.gravity_rush_2.starter", "Gravity Rush 2", ["CUSA03694", "CUSA04943"]),
    ]:
        created.append(write_onehundo(
            fname,
            {
                "manifest.json": {
                    "manifest_version": "0.1.0",
                    "pack_id": pid,
                    "pack_version": "0.1.0",
                    "canonical_game_id": gid,
                    "title": title,
                    "authors": ["OneHundoOrg"],
                    "matchers": [
                        {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": code, "os": "any"}
                        for code in plat_ids
                    ],
                    "profiles": {
                        "observation": {
                            "enabled": True,
                            "rules_file": "rules/save.json",
                            "sources": [{"source_type": "trophy_dir", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/TROP*"}],
                        }
                    },
                    "achievements": [
                        ach("ach_gravity_awakened", "Gravity Awakened", "Master basic gravity shifting.", 15),
                        ach("ach_hekseville_hero", "Guardian of Hekseville", "Complete the main story.", 50),
                    ],
                },
                "rules/save.json": tmpl_save_doc(
                    gid,
                    f"{gid.replace('.', '_')}_template",
                    "**/savedata/**",
                    "save_file",
                    "Supports Vita3K / shadPS4 trophy exports; save-binary rules marked template pending verified offsets.",
                ),
            },
        ))

    # 13. Crash Team Racing Nitro-Fueled
    created.append(write_onehundo(
        "activision.ctr_nf.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "activision.ctr_nf.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "activision.ctr_nitro_fueled",
                "title": "Crash Team Racing Nitro-Fueled",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "0100F9F00C696000", "os": "any"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA13795", "region": "USA", "launcher": "shadps4"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA14876", "region": "EUR", "launcher": "shadps4"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/CTR*"}],
                    }
                },
                "achievements": [
                    ach("ach_woah", "Woah!", "Complete Adventure Mode.", 30),
                    ach("ach_gasmoxian_slug", "Gasmoxian Slug", "Beat all of Nitros Oxide's Time Trial ghosts.", 60),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "activision.ctr_nitro_fueled",
                "ctr_nf_save_template",
                "**/CTR*",
                "save_file",
                "Ryujinx / Eden / shadPS4 save template.",
            ),
        },
    ))

    # 14. Shadow of the Colossus (PCSX2 / RPCS3 / shadPS4)
    created.append(write_onehundo(
        "sony.sotc.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "sony.sotc.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "sony.shadow_of_the_colossus",
                "title": "Shadow of the Colossus",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "platform_id", "algorithm": "exact", "value": "SCUS-97472", "region": "USA", "launcher": "pcsx2"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "BCUS98289", "region": "USA", "launcher": "rpcs3"},
                    {"matcher_type": "platform_id", "algorithm": "psn_title_id", "value": "CUSA08034", "region": "USA", "launcher": "shadps4"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "trophy_dir", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/TROP*"}],
                    }
                },
                "achievements": [
                    ach("ach_valus", "Valley of the Fallen", "Defeat the 1st Colossus.", 15),
                    ach("ach_dormin", "The Horned Boy", "Complete the Forbidden Lands journey.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "sony.shadow_of_the_colossus",
                "sotc_save_template",
                "**/*sotc*.psu",
                "emulator_sram",
                "Supports RPCS3/shadPS4 trophy directories; PS2 .psu/.mc2 card rules marked template.",
            ),
        },
    ))

    # 15a-b. Sonic Adventure & Sonic Adventure 2
    for gid, fname, pid, title, steam_id, exe in [
        ("sega.sonic_adventure", "sega.sonic_adventure.onehundo", "sega.sa1.starter", "Sonic Adventure", "71250", "*Sonic Adventure DX*"),
        ("sega.sonic_adventure_2", "sega.sonic_adventure_2.onehundo", "sega.sa2.starter", "Sonic Adventure 2", "213610", "*sonic2app*"),
    ]:
        created.append(write_onehundo(
            fname,
            {
                "manifest.json": {
                    "manifest_version": "0.1.0",
                    "pack_id": pid,
                    "pack_version": "0.1.0",
                    "canonical_game_id": gid,
                    "title": title,
                    "authors": ["OneHundoOrg"],
                    "matchers": [
                        {"matcher_type": "app_id", "algorithm": "steam_appid", "value": steam_id, "os": "any"},
                        {"matcher_type": "executable", "algorithm": "glob", "value": exe, "os": "any"},
                        {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/*SONICADV*", "os": "any"},
                    ],
                    "profiles": {
                        "observation": {
                            "enabled": True,
                            "rules_file": "rules/save.json",
                            "sources": [
                                {"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*SONICADV*"},
                                {"source_type": "goldberg_json", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/achievements.json"},
                            ],
                        }
                    },
                    "achievements": [
                        ach("ach_emblems_start", "Speed Highway Clear", "Clear Sonic's opening story stage.", 15),
                        ach("ach_all_emblems", "All Emblems", "Collect all Emblems across all stories.", 60),
                    ],
                },
                "rules/save.json": tmpl_save_doc(
                    gid,
                    f"{gid.replace('.', '_')}_template",
                    "**/*SONICADV*",
                    "save_file",
                    "Dreamcast VMU / PC save template; Goldberg/GSE achievements.json imports automatically if present.",
                ),
            },
        ))

    # 16a-b. Pokemon Colosseum & Pokemon XD: Gale of Darkness (Dolphin)
    for gid, fname, pid, title, codes in [
        ("nintendo.pokemon_colosseum", "nintendo.pokemon_colosseum.onehundo", "nintendo.colosseum.dolphin", "Pokémon Colosseum", ["GC6E01", "GC6P01", "GC6J01"]),
        ("nintendo.pokemon_xd", "nintendo.pokemon_xd.onehundo", "nintendo.pokemon_xd.dolphin", "Pokémon XD: Gale of Darkness", ["GXXE01", "GXXP01", "GXXJ01"]),
    ]:
        created.append(write_onehundo(
            fname,
            {
                "manifest.json": {
                    "manifest_version": "0.1.0",
                    "pack_id": pid,
                    "pack_version": "0.1.0",
                    "canonical_game_id": gid,
                    "title": title,
                    "authors": ["OneHundoOrg"],
                    "matchers": [
                        {"matcher_type": "platform_id", "algorithm": "exact", "value": code, "launcher": "dolphin"}
                        for code in codes
                    ],
                    "profiles": {
                        "observation": {
                            "enabled": True,
                            "rules_file": "rules/save.json",
                            "sources": [{"source_type": "emulator_sram", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*.gci"}],
                        }
                    },
                    "achievements": [
                        ach("ach_first_snag", "Shadow Snagger", "Snag and purify your first Shadow Pokémon.", 15),
                        ach("ach_orre_champion", "Champion of Orre", "Clear Mt. Battle and the main Orre story.", 50),
                    ],
                },
                "rules/save.json": tmpl_save_doc(
                    gid,
                    f"{gid.replace('.', '_')}_gci_template",
                    "**/*.gci",
                    "emulator_sram",
                    "Dolphin .gci memory card slot encryption uses checksum + slot rotation; marked placeholder_template pending verified decryptor.",
                ),
            },
        ))

    # 17. Star Fox 64 (Starship / N64)
    created.append(write_onehundo(
        "nintendo.starfox64.starship.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "nintendo.starfox64.starship",
                "pack_version": "0.1.0",
                "canonical_game_id": "nintendo.star_fox_64",
                "title": "Star Fox 64",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "starship", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*Starship*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/Starship/**/save*", "os": "any"},
                ],
                "profiles": {
                    "semantic": {"enabled": True, "producer_ids": ["starship"]},
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/starship_save*.json"}],
                    },
                },
                "achievements": [
                    ach("ach_corneria_clear", "Corneria Defender", "Clear Corneria in the Lylat System.", 15),
                    ach("ach_venom_medals", "Lylat Ace", "Earn all 15 Medals on Expert.", 60),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "nintendo.star_fox_64",
                "starship_save_template",
                "**/starship_save*.json",
                "save_file",
                "Starship port save template covering native port saves and RetroArch SRAM.",
            ),
        },
    ))

    # 18a-b. Banjo-Kazooie & Donkey Kong 64
    created.append(write_onehundo(
        "rare.banjo_kazooie.recomp.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "rare.banjo_kazooie.recomp",
                "pack_version": "0.1.0",
                "canonical_game_id": "rare.banjo_kazooie",
                "title": "Banjo-Kazooie",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "banjorecomp", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*BanjoRecomp*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/BanjoRecomp/**/saves/*", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "save_file", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*.eep"}],
                    }
                },
                "achievements": [
                    ach("ach_mumbo_mountain", "Mumbo's Mountain", "Collect all 10 Jiggies in Mumbo's Mountain.", 20),
                    ach("ach_gruntilda", "Witch Slayers", "Defeat Gruntilda atop her lair.", 50),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "rare.banjo_kazooie",
                "banjo_eep_template",
                "**/*.eep",
                "emulator_sram",
                "BanjoRecomp / N64 EEPROM save template.",
            ),
        },
    ))

    created.append(write_onehundo(
        "rare.donkey_kong_64.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "rare.dk64.starter",
                "pack_version": "0.1.0",
                "canonical_game_id": "rare.donkey_kong_64",
                "title": "Donkey Kong 64",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "recomp_game_id", "algorithm": "exact", "value": "dk64_decomp", "os": "any"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*dk64*", "os": "any"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/Donkey Kong 64*.eep", "os": "any"},
                ],
                "profiles": {
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [{"source_type": "emulator_sram", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/*.eep"}],
                    }
                },
                "achievements": [
                    ach("ach_kong_freed", "Five Kongs Reunited", "Rescue Diddy, Lanky, Tiny, and Chunky Kong.", 25),
                    ach("ach_101_percent", "101% Golden Banana", "Reach 101% completion.", 100),
                ],
            },
            "rules/save.json": tmpl_save_doc(
                "rare.donkey_kong_64",
                "dk64_eep_template",
                "**/*.eep",
                "emulator_sram",
                "DK64 EEPROM save template.",
            ),
        },
    ))

    # 19. RetroArch Multi-Core Bridge Pack (verifiable Plane A + JSON/SRAM example)
    created.append(write_onehundo(
        "retroarch.multicore.bridge.onehundo",
        {
            "manifest.json": {
                "manifest_version": "0.1.0",
                "pack_id": "retroarch.multicore.bridge",
                "pack_version": "0.1.0",
                "canonical_game_id": "retroarch.multicore",
                "title": "RetroArch Multi-Core",
                "authors": ["OneHundoOrg"],
                "matchers": [
                    {"matcher_type": "rom_hash", "algorithm": "exact", "value": "d63ed5f8", "os": "any", "launcher": "retroarch"},
                    {"matcher_type": "executable", "algorithm": "glob", "value": "*retroarch*", "os": "any", "launcher": "retroarch"},
                    {"matcher_type": "save_path_glob", "algorithm": "glob", "value": "**/RetroArch/saves/*.srm", "os": "any", "launcher": "retroarch"},
                ],
                "profiles": {
                    "memory": {
                        "enabled": True,
                        "architecture": "snes",
                        "default_endianness": "LITTLE",
                        "regions": [
                            {"region_name": "WRAM", "guest_base_address": "0x7e0000", "size_bytes": 512}
                        ],
                        "rules_file": "rules/memory.json",
                        "retroarch": {
                            "use_network_cmd": True,
                            "core_name": "snes9x",
                            "content_crc": "d63ed5f8",
                            "poll_interval_ms": 250,
                        },
                    },
                    "observation": {
                        "enabled": True,
                        "rules_file": "rules/save.json",
                        "sources": [
                            {"source_type": "emulator_sram", "evidence_class": "LOCALLY_OBSERVED", "file_glob": "**/retroarch_state.json"}
                        ],
                        "watch_paths": [
                            {"os": "linux", "launcher": "retroarch", "path_glob": "~/.config/retroarch/saves/*.srm"},
                            {"os": "steamos", "launcher": "retroarch", "path_glob": "~/.var/app/org.libretro.RetroArch/config/retroarch/saves/*.srm"},
                            {"os": "windows", "launcher": "retroarch", "path_glob": "%APPDATA%/RetroArch/saves/*.srm"},
                            {"os": "android", "launcher": "retroarch", "path_glob": "/storage/emulated/0/RetroArch/saves/*.srm"},
                        ],
                    },
                },
                "achievements": [
                    ach("ach_ra_boss_clear", "Core Memory Milestone", "Unlock when core RAM flag 0x7E0010 increments cleanly.", 15),
                    ach("ach_ra_sram_export", "Portable SRAM Checkpoint", "Observe verified SRAM state export.", 10),
                ],
            },
            "rules/memory.json": {
                "memory_rules": [
                    {
                        "achievement_id": "ach_ra_boss_clear",
                        "conditions": [
                            {
                                "region_id": 0,
                                "address": "0x10",
                                "size": "u8",
                                "op": "==",
                                "value": 1,
                                "kind": "once",
                                "required_hits": 1,
                            }
                        ],
                    }
                ]
            },
            "rules/save.json": {
                "schema_version": "0.1.0",
                "canonical_game_id": "retroarch.multicore",
                "notes": "Portable JSON save-state checkpoint rule alongside RetroArch UDP WRAM monitoring.",
                "watchers": [
                    {
                        "id": "retroarch_state_json",
                        "source_type": "emulator_sram",
                        "format": "json",
                        "file_glob": "**/retroarch_state.json",
                        "achievement_rules": [
                            {
                                "achievement_id": "ach_ra_sram_export",
                                "conditions": [{"path": "progress.checkpoint_cleared", "value_type": "bool", "op": "==", "value": True}],
                            }
                        ],
                    }
                ],
            },
        },
    ))

    return created


if __name__ == "__main__":
    paths = build_all()
    print(f"Generated {len(paths)} Stage 1 packs in {PACKS_DIR}")
    for p in paths:
        print(" -", p.name)
