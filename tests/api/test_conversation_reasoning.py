from pydantic import ValidationError
import pytest

from deeptutor.core.turn_request import TurnRequest
from deeptutor.services.model_selection.llm import LLMSelection


@pytest.mark.parametrize(
    "effort", ["none", "minimal", "low", "medium", "high", "xhigh", "max", "adaptive"]
)
def test_turn_protocol_preserves_conversation_reasoning(effort):
    request = TurnRequest.model_validate(
        {
            "content": "hello",
            "llm_selection": {"profile_id": "p1", "model_id": "m1", "reasoning_effort": effort},
        }
    )
    payload = request.model_dump()["llm_selection"]
    assert LLMSelection.from_payload(payload).reasoning_effort == effort


def test_turn_protocol_rejects_invalid_effort_and_preserves_legacy_defaults():
    with pytest.raises(ValidationError):
        TurnRequest.model_validate(
            {
                "content": "hello",
                "llm_selection": {
                    "profile_id": "p1",
                    "model_id": "m1",
                    "reasoning_effort": "invalid",
                },
            }
        )
    request = TurnRequest.model_validate(
        {
            "content": "hello",
            "llm_selection": {
                "profile_id": "p1",
                "model_id": "m1",
            },
        }
    )
    assert request.llm_selection.reasoning_effort is None
