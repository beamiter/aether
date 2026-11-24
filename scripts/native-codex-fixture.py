#!/usr/bin/python3
"""Local deterministic app-server peer installed only in the native smoke PATH."""
import json
import os
from pathlib import Path
import sys
import uuid

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib


arguments = sys.argv[1:]
root = Path(__file__).resolve().parents[1]
if arguments and arguments[0] == "resume":
    legacy = Path(__file__).with_name("codex-cli-fixture")
    os.execv(legacy, [str(legacy), *arguments])
if arguments == ["--version"]:
    print("codex-cli 0.147.0")
    sys.exit()
if not arguments or arguments[0] != "app-server":
    sys.exit("unexpected fixture invocation")

features = {value: False for flag, value in zip(arguments, arguments[1:]) if flag == "--disable"}
config = tomllib.loads("\n".join(value for flag, value in zip(arguments, arguments[1:]) if flag == "-c"))
config["features"] = features
profile = Path(os.environ["CODEX_HOME"])
fixture_thread = str(uuid.uuid4())
fixture_turn = "controlled-turn"
approval_rpc = 89


def send(value):
    print(json.dumps(value), flush=True)


def record(name, value):
    with (root / name).open("a") as output:
        output.write(json.dumps(value) + "\n")


def attestation():
    tool_environment = config["shell_environment_policy"]["set"]
    origin = {"name": {"type": "sessionFlags"}, "version": "fixture"}
    fields = ["web_search", "allow_login_shell", "shell_environment_policy.inherit",
              "shell_environment_policy.ignore_default_excludes"]
    fields.extend(f"features.{feature}" for feature in features)
    fields.extend(f"shell_environment_policy.set.{key}" for key in tool_environment)
    return {
        "config": {"mcp_servers": {}, "plugins": {}, "marketplaces": {}, **config},
        "layers": [
            {"name": {"type": "sessionFlags"}, "config": config, "version": "fixture"},
            {"name": {"type": "user", "file": str(profile / "config.toml")}, "config": {}, "version": "fixture"},
            {"name": {"type": "system"}, "config": {}, "version": "fixture"},
        ],
        "origins": dict.fromkeys(fields, origin),
    }


def delta(text):
    send({"method": "item/agentMessage/delta", "params": {
        "threadId": fixture_thread, "turnId": fixture_turn, "itemId": "fixture-message", "delta": text,
    }})


record("native-provider-starts", {"pid": os.getpid(), "cwd": os.getcwd(), "thread": fixture_thread,
                                  "cgroup": Path("/proc/self/cgroup").read_text().strip()})
for line in sys.stdin:
    request = json.loads(line)
    rpc = request.get("id")
    method = request.get("method")
    params = request.get("params", {})
    if rpc == approval_rpc and not method:
        assert request["result"] == {"decision": "decline"}
        record("native-approval-receipts", request["result"])
        delta("Decline observed by the scoped provider.")
        send({"method": "turn/completed", "params": {
            "threadId": fixture_thread, "turn": {"id": fixture_turn, "status": "completed"},
        }})
        continue
    if method == "initialize":
        name = params["clientInfo"]["name"]
        result = {"userAgent": f"{name}/0.147.0 (fixture)", "codexHome": str(profile)}
    elif method == "initialized":
        continue
    elif method == "config/read":
        assert params["includeLayers"] is True
        result = attestation()
    elif method == "account/login/start":
        assert params["type"] == "chatgptAuthTokens" and params["accessToken"] == "native-fixture-placeholder"
        result = {"type": "chatgptAuthTokens"}
    elif method == "thread/start":
        assert params["approvalPolicy"] == "never" and params["sandbox"] == "read-only"
        result = {"thread": {"id": fixture_thread}}
    elif method == "turn/start":
        assert params["threadId"] == fixture_thread and params["approvalPolicy"] == "never"
        prompt = params["input"][0]["text"]
        assert "Review the fixture approval" in prompt
        brief = next(json.loads(line) for line in prompt.splitlines() if line.startswith('{"goal":'))
        record("native-task-prompts", {"pid": os.getpid(), "brief": brief})
        Path("tracked.txt").write_text("edited inside the native fixture worktree\n")
        send({"id": rpc, "result": {"turn": {"id": fixture_turn}}})
        delta("The provider has a pending request. ")
        send({"id": approval_rpc, "method": "item/commandExecution/requestApproval", "params": {
            "threadId": fixture_thread, "turnId": fixture_turn, "itemId": "fixture-command",
            "startedAtMs": 1, "command": "printf fixture-only", "cwd": str(Path.cwd()),
        }})
        continue
    else:
        sys.exit(f"unexpected fixture method: {method}")
    send({"id": rpc, "result": result})
