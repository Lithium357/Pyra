---
pipeline_tag: object-detection
tags:
- art
- anime
license: openrail
library_name: dghs-imgutils
metrics:
- f1
datasets:
- deepghs/anime_eye_detection
---

|       Model       |  FLOPS  |  Params  |  F1 Score  |  Threshold  |                                               F1 Plot                                               |                                                    Confusion                                                     |  Labels  |
|:-----------------:|:-------:|:--------:|:----------:|:-----------:|:---------------------------------------------------------------------------------------------------:|:----------------------------------------------------------------------------------------------------------------:|:--------:|
| eye_detect_v1.0_n |   898   |  3.01M   |    0.91    |    0.258    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v1.0_n/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v1.0_n/confusion_matrix.png) |  `eye`   |
| eye_detect_v1.0_s |  3.49k  |  11.1M   |    0.93    |    0.228    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v1.0_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v1.0_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.8_s |  3.49k  |  11.1M   |    0.93    |    0.309    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.8_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.8_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.7_s |  3.49k  |  11.1M   |    0.93    |    0.275    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.7_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.7_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.6_s |  3.49k  |  11.1M   |    0.93    |    0.333    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.6_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.6_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.5_s |  3.49k  |  11.1M   |    0.93    |    0.26     | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.5_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.5_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.4_s |  3.49k  |  11.1M   |    0.94    |    0.32     | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.4_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.4_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.3_s |  3.49k  |  11.1M   |    0.85    |    0.305    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.3_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.3_s/confusion_matrix.png) |  `eye`   |
| eye_detect_v0.2_s |  3.49k  |  11.1M   |    0.84    |    0.339    | [plot](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.2_s/F1_curve.png) | [confusion](https://huggingface.co/deepghs/anime_eye_detection/blob/main/eye_detect_v0.2_s/confusion_matrix.png) |  `eye`   |