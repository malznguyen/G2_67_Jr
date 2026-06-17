#!/usr/bin/env sh
# =========================================================
# G2_67_Jr — MinIO bootstrap
# Creates the gmrag-uploads bucket and sets a sane default policy.
# Runs inside the minio container after healthcheck passes.
# =========================================================

set -eu

ENDPOINT="${S3_ENDPOINT:-http://minio:9000}"
ACCESS_KEY="${S3_ACCESS_KEY:?S3_ACCESS_KEY is required}"
SECRET_KEY="${S3_SECRET_KEY:?S3_SECRET_KEY is required}"
BUCKET="${S3_BUCKET:-gmrag-uploads}"
REGION="${S3_REGION:-us-east-1}"

# Use the official MinIO client (mc) shipped in the minio image.
# `mc` is available at /usr/bin/mc inside the minio container.
mc --version >/dev/null

mc alias set gmrag "${ENDPOINT}" "${ACCESS_KEY}" "${SECRET_KEY}" --api S3v4

if mc ls "gmrag/${BUCKET}" >/dev/null 2>&1; then
  echo "[minio-init] bucket '${BUCKET}' already exists — skipping create"
else
  mc mb --region "${REGION}" "gmrag/${BUCKET}"
  echo "[minio-init] bucket '${BUCKET}' created"
fi

# Default policy: private. Uploads go through signed URLs from the backend.
mc anonymous set none "gmrag/${BUCKET}"

echo "[minio-init] done: bucket=${BUCKET} region=${REGION} endpoint=${ENDPOINT}"