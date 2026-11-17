"""Document metadata, with the PDF engine."""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from PySide6.QtCore import QXmlStreamReader

if TYPE_CHECKING:
    from collections.abc import Mapping

    from plain_open_pdf.adapters.document.engine.document import EngineDocument


#: The app's metadata keys and the information dictionary keys they stand
#: for; "format" and "encryption" are read from the document itself.
_INFO_PDF_KEYS = {
    "title": "Title",
    "author": "Author",
    "subject": "Subject",
    "keywords": "Keywords",
    "creator": "Creator",
    "producer": "Producer",
    "creationDate": "CreationDate",
    "modDate": "ModDate",
    "trapped": "Trapped",
}


_INFO_METADATA_KEYS = (*_INFO_PDF_KEYS, "format", "encryption")


_STANDARD_INFO_PDF_KEYS = frozenset(_INFO_PDF_KEYS.values())


_CUSTOM_INFO_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+$")


_PDF_DATE_RE = re.compile(
    r"^D:"
    r"\d{4}"
    r"(\d{2}"
    r"(\d{2}"
    r"(\d{2}"
    r"(\d{2}"
    r"(\d{2}"
    r"((Z|[+-]\d{2}'\d{2}'))?"
    r")?)?)?)?)?$",
)


_TRAPPED_VALUES = frozenset({"", "True", "False", "Unknown"})


_XPACKET_PI_RE = re.compile(r"<\?xpacket\b[^?]*\?>", re.IGNORECASE)


def _information(document: EngineDocument) -> dict[str, str]:
    """Return every text entry of the information dictionary."""
    entries = document.native.information_entries()
    return {str(key): str(value) for key, value in entries.items()}


def _custom_info_keys(information: Mapping[str, str]) -> list[str]:
    return sorted(
        key
        for key in information
        if key not in _STANDARD_INFO_PDF_KEYS
        and _CUSTOM_INFO_NAME_RE.fullmatch(key) is not None
    )


def read_document_metadata(
    document: EngineDocument,
) -> tuple[dict[str, str], dict[str, str], str]:
    """Return standard, custom, and raw XMP document metadata."""
    information = _information(document)
    info_dict = {
        key: information.get(pdf_key, "")
        for key, pdf_key in _INFO_PDF_KEYS.items()
    }
    info_dict["format"] = f"PDF {document.native.version}"
    info_dict["encryption"] = document.native.encryption or ""
    custom_info_dict = {
        key: information[key] for key in _custom_info_keys(information)
    }
    xmp_xml = document.native.xmp_metadata() or ""
    return info_dict, custom_info_dict, xmp_xml


def write_document_metadata(
    document: EngineDocument,
    *,
    info_dict: Mapping[str, str],
    custom_info_dict: Mapping[str, str],
    xmp_xml: str,
) -> None:
    """Update standard, custom, and raw XMP document metadata.

    An empty value removes its entry.
    """
    _validate_metadata(
        info_dict=info_dict,
        custom_info_dict=custom_info_dict,
        xmp_xml=xmp_xml,
    )
    native = document.native
    for key, pdf_key in _INFO_PDF_KEYS.items():
        native.set_information(pdf_key, info_dict.get(key, "") or None)

    for removed in set(_custom_info_keys(_information(document))) - set(
        custom_info_dict,
    ):
        native.set_information(removed, None)
    for name, value in custom_info_dict.items():
        native.set_information(name, value)

    native.set_xmp_metadata(xmp_xml or None)


def _validate_metadata(
    *,
    info_dict: Mapping[str, str],
    custom_info_dict: Mapping[str, str],
    xmp_xml: str,
) -> None:
    for date_key in ("creationDate", "modDate"):
        _validate_pdf_date(date_key, info_dict.get(date_key, ""))

    trapped = info_dict.get("trapped", "")
    if trapped not in _TRAPPED_VALUES:
        msg = "trapped must be True, False, or Unknown."
        raise ValueError(msg)

    _validate_custom_info_metadata(custom_info_dict)

    if xmp_xml:
        _validate_xmp_xml(xmp_xml)


def _validate_custom_info_metadata(
    custom_info_dict: Mapping[str, str],
) -> None:
    for name, value in custom_info_dict.items():
        if not name:
            msg = "Custom property names cannot be empty."
            raise ValueError(msg)
        if _CUSTOM_INFO_NAME_RE.fullmatch(name) is None:
            msg = (
                "Custom property names may contain only letters, numbers, "
                "periods, underscores, and hyphens."
            )
            raise ValueError(msg)
        if name in _STANDARD_INFO_PDF_KEYS:
            msg = f"{name} is a standard PDF property name."
            raise ValueError(msg)
        if not isinstance(value, str):
            msg = f"Custom property {name} must have a text value."
            raise TypeError(msg)


def _validate_pdf_date(field_name: str, date_text: str) -> None:
    if not date_text:
        return

    if _PDF_DATE_RE.fullmatch(date_text):
        return

    msg = f"{field_name} must use PDF date syntax."
    raise ValueError(msg)


def _validate_xmp_xml(xmp_xml: str) -> None:
    xml_body = _XPACKET_PI_RE.sub("", xmp_xml).strip()
    reader = QXmlStreamReader(xml_body)
    saw_start_element = False
    while not reader.atEnd():
        token = reader.readNext()
        if token == QXmlStreamReader.TokenType.StartElement:
            saw_start_element = True

    if reader.hasError() or not saw_start_element:
        msg = "XMP metadata must be valid XML."
        raise ValueError(msg)
