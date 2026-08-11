{{- define "pitools.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "pitools.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "pitools.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | quote }}
{{ include "pitools.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "pitools.selectorLabels" -}}
app.kubernetes.io/name: {{ include "pitools.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "pitools.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "pitools.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- required "serviceAccount.name is required when serviceAccount.create is false" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "pitools.image" -}}
{{- $repository := required "image.repository is required" .Values.image.repository -}}
{{- if .Values.image.digest -}}
{{- printf "%s@%s" $repository .Values.image.digest -}}
{{- else -}}
{{- printf "%s:%s" $repository (required "image.tag or image.digest is required" .Values.image.tag) -}}
{{- end -}}
{{- end }}

{{- define "pitools.piWorkerImage" -}}
{{- $repository := required "piWorker.image.repository is required" .Values.piWorker.image.repository -}}
{{- if .Values.piWorker.image.digest -}}
{{- printf "%s@%s" $repository .Values.piWorker.image.digest -}}
{{- else -}}
{{- printf "%s:%s" $repository (required "piWorker.image.tag or piWorker.image.digest is required" .Values.piWorker.image.tag) -}}
{{- end -}}
{{- end }}
